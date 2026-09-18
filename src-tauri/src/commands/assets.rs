//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.

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
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<CatalogConfigRow>, IpcError> {
    catalog::config(&store)
}

#[tauri::command]
pub fn catalog_configure(
    args: ConfigureArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CatalogConfigRow, IpcError> {
    catalog::configure(args, &store)
}

#[tauri::command]
pub fn catalog_load(
    args: LoadArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::events::CatalogSummary, IpcError> {
    catalog::load(args.pull, &store)
}

#[tauri::command]
pub fn catalog_list_assets(store: State<'_, Arc<Mutex<Store>>>) -> Result<AssetListing, IpcError> {
    catalog::list_assets(&store)
}

#[tauri::command]
pub fn catalog_get_asset(
    args: GetAssetArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AssetDetail, IpcError> {
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
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::LayerListing, IpcError> {
    catalog::list_layers(&store)
}

#[tauri::command]
pub fn catalog_resolve_preview(
    args: ResolvePreviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::resolve::Resolution, IpcError> {
    catalog::resolve_preview(&args.host_alias, &store)
}

#[tauri::command]
pub fn catalog_propose_layers(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::propose::LayerProposal, IpcError> {
    catalog::propose::propose_layers(&store)
}

#[tauri::command]
pub fn catalog_set_host_layers(
    args: SetHostLayersArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostLayerRow>, IpcError> {
    catalog::set_host_layers(
        &args.host_alias,
        args.role.as_deref(),
        &args.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
        &store,
    )
}

#[tauri::command]
pub fn catalog_layer_template(args: LayerTemplateArgs) -> Result<catalog::layer::Layer, IpcError> {
    check_layer_name(&args.name)?;
    Ok(author::layer_template(&args.name, args.axis))
}

#[tauri::command]
pub fn catalog_write_layer(
    args: WriteLayerArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    check_layer_name(&args.layer.name)?;
    author::write_layer(&args.layer, &store)
}

#[tauri::command]
pub fn catalog_delete_layer(
    args: LayerRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    check_layer_name(&args.name)?;
    author::delete_layer(&args.name, &store)
}

#[tauri::command]
pub fn catalog_import_host(
    args: ImportArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ImportReport, IpcError> {
    let token = {
        let s = lock(&store)?;
        s.get_setting(fleet_core::mcp::SETTING_TOKEN)?
    };
    catalog::import_host(args, &store, token.as_deref())
}

#[tauri::command]
pub async fn assets_scan_hosts(
    args: ScanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<inventory::HostScanResult>, IpcError> {
    inventory::scan_hosts(&store, &ssh, args.host_alias.as_deref()).await
}

#[tauri::command]
pub fn assets_inventory(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AssetInventoryRow>, IpcError> {
    catalog::inventory(&store)
}

#[tauri::command]
pub async fn catalog_plan_sync(
    args: PlanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SyncPlan, IpcError> {
    sync::plan_sync(args, &store, &ssh).await
}

#[tauri::command]
pub async fn catalog_apply_sync(
    args: ApplyArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SyncRunSummary, IpcError> {
    sync::apply_sync(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub fn catalog_last_sync(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<SyncRunSummary>, IpcError> {
    sync::last_sync(&store)
}

#[tauri::command]
pub fn catalog_list_secrets(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SecretRow>, IpcError> {
    let s = lock(&store)?;
    Ok(s.list_secrets()?)
}

#[tauri::command]
pub fn catalog_set_secret(
    args: SetSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
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
    args: DeleteSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    let s = lock(&store)?;
    Ok(s.delete_secret(&args.name, args.host_alias.as_deref())?)
}

// ------------------------------------------------------------- authoring

#[tauri::command]
pub fn catalog_create_asset(
    args: CreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    if let Some(from) = args.duplicate_from.as_deref() {
        check_name(from)?;
    }
    author::create(args, &store)
}

#[tauri::command]
pub fn catalog_update_asset(
    args: UpdateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.asset.header.name)?;
    for r in &args.asset.resources {
        check_resource_path(&r.rel_path)?;
    }
    author::update(args, &store)
}

#[tauri::command]
pub fn catalog_delete_asset(
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    check_name(&args.name)?;
    author::delete_asset(args, &store)
}

#[tauri::command]
pub fn catalog_add_resource(
    args: AddResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    check_local_path(&args.local_path)?;
    if let Some(rel_path) = args.rel_path.as_deref() {
        check_resource_path(rel_path)?;
    }
    author::add_resource(args, &store)
}

#[tauri::command]
pub fn catalog_remove_resource(
    args: RemoveResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    check_name(&args.name)?;
    check_resource_path(&args.rel_path)?;
    author::remove_resource(args, &store)
}

#[tauri::command]
pub fn catalog_lint_asset(
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LintReport, IpcError> {
    check_name(&args.name)?;
    author::lint_asset(args, &store)
}

#[tauri::command]
pub fn catalog_lint_all(store: State<'_, Arc<Mutex<Store>>>) -> Result<LintAll, IpcError> {
    author::lint_everything(&store)
}

#[tauri::command]
pub fn catalog_commit_pending(
    args: CommitPendingArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    author::commit_pending(args, &store)
}

#[tauri::command]
pub fn catalog_push(store: State<'_, Arc<Mutex<Store>>>) -> Result<RepoStatus, IpcError> {
    author::push(&store)
}

#[tauri::command]
pub fn catalog_repo_status(store: State<'_, Arc<Mutex<Store>>>) -> Result<RepoStatus, IpcError> {
    author::repo_status(&store)
}

#[tauri::command]
pub fn catalog_template(args: AssetRef) -> Result<Asset, IpcError> {
    check_name(&args.name)?;
    Ok(author::template(args.kind, &args.name))
}

#[tauri::command]
pub async fn catalog_spawn_author_session(
    args: SpawnAuthorArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    if let Some(name) = args.name.as_deref() {
        check_name(name)?;
    }
    author_session::spawn_author_session(args, &store, &ssh, &reg).await
}
