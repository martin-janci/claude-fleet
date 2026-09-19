//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::catalog::{
    self,
    author::{
        self, AddResourceArgs, AssetRef, CommitPendingArgs, CreateArgs, LintAll, LintReport,
        RemoveResourceArgs, UpdateArgs, WriteResult,
    },
    author_session::{self, SpawnAuthorArgs},
    import::ImportReport,
    inventory,
    model::{is_valid_name, Asset, Kind},
    repo::RepoStatus,
    sync::{
        self, plan::SyncPlan, secrets::is_valid_secret_name, ApplyArgs, PlanArgs, SyncRunSummary,
    },
    AssetDetail, AssetListing, ConfigureArgs, ImportArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::{
    AssetInventoryRow, CatalogConfigRow, HostLayerRow, SecretRow, SessionRow, Store,
};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::State;

fn invalid_name(name: &str) -> IpcError {
    IpcError::new(codes::E_INVALID, format!("invalid asset name '{name}'"))
}

fn check_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(invalid_name(name))
    }
}

fn check_layer_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid layer name '{name}'"),
        ))
    }
}

fn check_resource_path(rel_path: &str) -> Result<(), IpcError> {
    if catalog::repo::valid_resource_rel_path(rel_path) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid resource path: {rel_path}"),
        ))
    }
}

/// `local_path` must be an absolute path to an existing regular file
/// (`symlink_metadata`, so a symlink is rejected rather than followed).
fn check_local_path(local_path: &str) -> Result<(), IpcError> {
    let p = Path::new(local_path);
    let is_regular_file = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file());
    if p.is_absolute() && is_regular_file {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("{local_path} is not an absolute path to an existing regular file"),
        ))
    }
}

#[derive(serde::Deserialize)]
pub struct SetSecretArgs {
    pub name: String,
    #[serde(default)]
    pub host_alias: Option<String>,
    pub value: String,
}

#[derive(serde::Deserialize)]
pub struct DeleteSecretArgs {
    pub name: String,
    #[serde(default)]
    pub host_alias: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct LoadArgs {
    #[serde(default)]
    pub pull: bool,
}

#[derive(serde::Deserialize)]
pub struct GetAssetArgs {
    pub kind: Kind,
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct ScanArgs {
    pub host_alias: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ResolvePreviewArgs {
    pub host_alias: String,
}

#[derive(serde::Deserialize)]
pub struct SetHostLayersArgs {
    pub host_alias: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub contexts: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct LayerTemplateArgs {
    pub name: String,
    pub axis: catalog::layer::Axis,
}

#[derive(serde::Deserialize)]
pub struct WriteLayerArgs {
    pub layer: catalog::layer::Layer,
}

#[derive(serde::Deserialize)]
pub struct LayerRef {
    pub name: String,
}

#[tauri::command]
pub fn catalog_config(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<CatalogConfigRow>, IpcError> {
    backend.local_only(
        "catalog_config",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    catalog::config(&store)
}

#[tauri::command]
pub fn catalog_configure(
    backend: State<'_, Arc<FleetBackend>>,
    args: ConfigureArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CatalogConfigRow, IpcError> {
    backend.local_only(
        "catalog_configure",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    catalog::configure(args, &store)
}

#[tauri::command]
pub fn catalog_load(
    backend: State<'_, Arc<FleetBackend>>,
    args: LoadArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::events::CatalogSummary, IpcError> {
    backend.local_only(
        "catalog_load",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    catalog::load(args.pull, &store)
}

#[tauri::command]
pub fn catalog_list_assets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AssetListing, IpcError> {
    backend.local_only(
        "catalog_list_assets",
        "the hub does serve this list (its read-only list_assets tool, \
         open to any paired client), but the Assets panel is built on the \
         catalog's configuration and git checkout, which only the machine \
         that owns the fleet has; call list_assets on the hub, or browse the \
         catalog on that machine",
    )?;
    catalog::list_assets(&store)
}

#[tauri::command]
pub fn catalog_get_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: GetAssetArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AssetDetail, IpcError> {
    backend.local_only(
        "catalog_get_asset",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    if !catalog::model::is_valid_name(&args.name) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid asset name '{}'", args.name),
        ));
    }
    catalog::get_asset(args.kind, &args.name, &store)
}

#[tauri::command]
pub fn catalog_list_layers(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::LayerListing, IpcError> {
    backend.local_only(
        "catalog_list_layers",
        "the hub does serve this (its read-only list_layers tool), but the \
         layer definitions live in the catalog's git checkout, which only the \
         machine that owns the fleet has; call list_layers on the hub, or \
         work on the catalog there",
    )?;
    catalog::list_layers(&store)
}

#[tauri::command]
pub fn catalog_resolve_preview(
    backend: State<'_, Arc<FleetBackend>>,
    args: ResolvePreviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::resolve::Resolution, IpcError> {
    backend.local_only(
        "catalog_resolve_preview",
        "the hub has a resolve_preview tool, but it answers a summary — kind, \
         name and version per asset — while this command returns the full \
         Resolution the UI renders, so routing it would silently drop every \
         asset body; call resolve_preview on the hub for the summary, or \
         resolve on the machine that owns the fleet",
    )?;
    catalog::resolve_preview(&args.host_alias, &store)
}

#[tauri::command]
pub fn catalog_propose_layers(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::propose::LayerProposal, IpcError> {
    backend.local_only(
        "catalog_propose_layers",
        "the hub does serve this (its read-only propose_layers tool), but a \
         proposal is only useful where the layers can then be written — the \
         catalog's git checkout, which only the machine that owns the fleet \
         has; call propose_layers on the hub, or propose on that machine",
    )?;
    catalog::propose::propose_layers(&store)
}

#[tauri::command]
pub fn catalog_set_host_layers(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetHostLayersArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostLayerRow>, IpcError> {
    backend.local_only(
        "catalog_set_host_layers",
        "the hub has a set_host_layers tool, but it is master-only — a host's \
         layer assignment decides what the next apply_sync writes to its \
         filesystem — and a paired client is never the master; set layers on \
         the machine that owns the fleet",
    )?;
    catalog::set_host_layers(
        &args.host_alias,
        args.role.as_deref(),
        &args.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
        &store,
    )
}

#[tauri::command]
pub fn catalog_layer_template(
    backend: State<'_, Arc<FleetBackend>>,
    args: LayerTemplateArgs,
) -> Result<catalog::layer::Layer, IpcError> {
    backend.local_only(
        "catalog_layer_template",
        "a template is the first step of authoring a layer into the \
         catalog's git checkout, and catalog_write_layer refuses here for \
         want of that checkout; the hub exposes no layer-authoring tool, so \
         author on the machine that owns the fleet",
    )?;
    check_layer_name(&args.name)?;
    Ok(author::layer_template(&args.name, args.axis))
}

#[tauri::command]
pub fn catalog_write_layer(
    backend: State<'_, Arc<FleetBackend>>,
    args: WriteLayerArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    backend.local_only(
        "catalog_write_layer",
        "writing a layer edits a file in the catalog's git checkout, which \
         only the machine that owns the fleet has, and the hub exposes no \
         layer-authoring tool; author on that machine",
    )?;
    check_layer_name(&args.layer.name)?;
    author::write_layer(&args.layer, &store)
}

#[tauri::command]
pub fn catalog_delete_layer(
    backend: State<'_, Arc<FleetBackend>>,
    args: LayerRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    backend.local_only(
        "catalog_delete_layer",
        "deleting a layer removes a file from the catalog's git checkout, \
         which only the machine that owns the fleet has, and the hub exposes \
         no layer-authoring tool; author on that machine",
    )?;
    check_layer_name(&args.name)?;
    author::delete_layer(&args.name, &store)
}

#[tauri::command]
pub fn catalog_import_host(
    backend: State<'_, Arc<FleetBackend>>,
    args: ImportArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ImportReport, IpcError> {
    backend.local_only(
        "catalog_import_host",
        "the hub has this as its import_assets tool, but the import \
         lands in the catalog's git checkout, which only the machine that \
         owns the fleet has; call import_assets on the hub, or import on \
         that machine",
    )?;
    let token = {
        let s = lock(&store)?;
        s.get_setting(fleet_core::mcp::SETTING_TOKEN)?
    };
    catalog::import_host(args, &store, token.as_deref())
}

#[tauri::command]
pub async fn assets_scan_hosts(
    backend: State<'_, Arc<FleetBackend>>,
    args: ScanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<inventory::HostScanResult>, IpcError> {
    backend.local_only(
        "assets_scan_hosts",
        "the hub has this as its scan_assets tool, but its result feeds \
         an inventory panel built on the catalog checkout, which only the \
         machine that owns the fleet has; call scan_assets on the hub, or \
         scan from that machine",
    )?;
    inventory::scan_hosts(&store, &ssh, args.host_alias.as_deref()).await
}

#[tauri::command]
pub fn assets_inventory(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AssetInventoryRow>, IpcError> {
    backend.local_only(
        "assets_inventory",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    catalog::inventory(&store)
}

#[tauri::command]
pub async fn catalog_plan_sync(
    backend: State<'_, Arc<FleetBackend>>,
    args: PlanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SyncPlan, IpcError> {
    backend.local_only(
        "catalog_plan_sync",
        "the hub has this as its plan_sync tool, but the plan is shown \
         in a sync panel built on the catalog checkout, which only the \
         machine that owns the fleet has; call plan_sync on the hub, or \
         plan on that machine",
    )?;
    sync::plan_sync(args, &store, &ssh).await
}

#[tauri::command]
pub async fn catalog_apply_sync(
    backend: State<'_, Arc<FleetBackend>>,
    args: ApplyArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SyncRunSummary, IpcError> {
    backend.local_only(
        "catalog_apply_sync",
        "the hub's apply_sync is master-only: a paired client is never \
         the fleet's administrator, and a sync writes to every host over \
         SSH; run the sync on the hub",
    )?;
    sync::apply_sync(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub fn catalog_last_sync(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<SyncRunSummary>, IpcError> {
    backend.local_only(
        "catalog_last_sync",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    sync::last_sync(&store)
}

#[tauri::command]
pub fn catalog_list_secrets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SecretRow>, IpcError> {
    backend.local_only(
        "catalog_list_secrets",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    let s = lock(&store)?;
    Ok(s.list_secrets()?)
}

#[tauri::command]
pub fn catalog_set_secret(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.local_only(
        "catalog_set_secret",
        "the hub's set_secret is master-only: a paired client is never \
         the fleet's administrator, and the sync secrets belong to the \
         machine that runs the sync; set it on the hub",
    )?;
    if !is_valid_secret_name(&args.name) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid secret name '{}'; use [A-Z0-9_]+", args.name),
        ));
    }
    let s = lock(&store)?;
    Ok(s.set_secret(&args.name, args.host_alias.as_deref(), &args.value)?)
}

#[tauri::command]
pub fn catalog_delete_secret(
    backend: State<'_, Arc<FleetBackend>>,
    args: DeleteSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    backend.local_only(
        "catalog_delete_secret",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    let s = lock(&store)?;
    Ok(s.delete_secret(&args.name, args.host_alias.as_deref())?)
}

// ------------------------------------------------------------- authoring

#[tauri::command]
pub fn catalog_create_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: CreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    backend.local_only(
        "catalog_create_asset",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    if let Some(from) = args.duplicate_from.as_deref() {
        check_name(from)?;
    }
    author::create(args, &store)
}

#[tauri::command]
pub fn catalog_update_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    backend.local_only(
        "catalog_update_asset",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.asset.header.name)?;
    for r in &args.asset.resources {
        check_resource_path(&r.rel_path)?;
    }
    author::update(args, &store)
}

#[tauri::command]
pub fn catalog_delete_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    backend.local_only(
        "catalog_delete_asset",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    author::delete_asset(args, &store)
}

#[tauri::command]
pub fn catalog_add_resource(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    backend.local_only(
        "catalog_add_resource",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    check_local_path(&args.local_path)?;
    if let Some(rel_path) = args.rel_path.as_deref() {
        check_resource_path(rel_path)?;
    }
    author::add_resource(args, &store)
}

#[tauri::command]
pub fn catalog_remove_resource(
    backend: State<'_, Arc<FleetBackend>>,
    args: RemoveResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    backend.local_only(
        "catalog_remove_resource",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    check_resource_path(&args.rel_path)?;
    author::remove_resource(args, &store)
}

#[tauri::command]
pub fn catalog_lint_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LintReport, IpcError> {
    backend.local_only(
        "catalog_lint_asset",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    author::lint_asset(args, &store)
}

#[tauri::command]
pub fn catalog_lint_all(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LintAll, IpcError> {
    backend.local_only(
        "catalog_lint_all",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    author::lint_everything(&store)
}

#[tauri::command]
pub fn catalog_commit_pending(
    backend: State<'_, Arc<FleetBackend>>,
    args: CommitPendingArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    backend.local_only(
        "catalog_commit_pending",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    author::commit_pending(args, &store)
}

#[tauri::command]
pub fn catalog_push(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    backend.local_only(
        "catalog_push",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    author::push(&store)
}

#[tauri::command]
pub fn catalog_repo_status(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    backend.local_only(
        "catalog_repo_status",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    author::repo_status(&store)
}

#[tauri::command]
pub fn catalog_template(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
) -> Result<Asset, IpcError> {
    backend.local_only(
        "catalog_template",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    check_name(&args.name)?;
    Ok(author::template(args.kind, &args.name))
}

#[tauri::command]
pub async fn catalog_spawn_author_session(
    backend: State<'_, Arc<FleetBackend>>,
    args: SpawnAuthorArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    backend.local_only(
        "catalog_spawn_author_session",
        "the asset catalog is a git checkout on the machine that owns the \
         fleet, and the hub has no tool for this; work on the catalog \
         there",
    )?;
    if let Some(name) = args.name.as_deref() {
        check_name(name)?;
    }
    author_session::spawn_author_session(args, &store, &ssh, &reg).await
}
