//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.

use crate::ipc_error::IpcError;
use crate::service::catalog::{
    self, import::ImportReport, inventory, model::Kind, AssetDetail, AssetListing, ConfigureArgs,
    ImportArgs,
};
use crate::ssh::SshClient;
use crate::store::{AssetInventoryRow, CatalogConfigRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

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
            "E_VALIDATION",
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
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
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
