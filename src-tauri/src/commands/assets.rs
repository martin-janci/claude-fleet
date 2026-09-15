//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, IpcError};
use crate::service::catalog::{
    self,
    import::ImportReport,
    inventory,
    model::Kind,
    sync::{
        self, plan::SyncPlan, secrets::is_valid_secret_name, ApplyArgs, PlanArgs, SyncRunSummary,
    },
    AssetDetail, AssetListing, ConfigureArgs, ImportArgs,
};
use crate::ssh::SshClient;
use crate::store::{AssetInventoryRow, CatalogConfigRow, SecretRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

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
) -> Result<crate::events::CatalogSummary, IpcError> {
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
pub fn catalog_import_host(
    args: ImportArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ImportReport, IpcError> {
    let token = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        s.get_setting(crate::mcp::SETTING_TOKEN)?
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
    let s = store.lock().map_err(|_| IpcError::lock())?;
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
    let s = store.lock().map_err(|_| IpcError::lock())?;
    Ok(s.set_secret(&args.name, args.host_alias.as_deref(), &args.value)?)
}

#[tauri::command]
pub fn catalog_delete_secret(
    args: DeleteSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    Ok(s.delete_secret(&args.name, args.host_alias.as_deref())?)
}
