//! Tauri IPC wrappers for account usage (Task 4). Logic lives in
//! `service::account_usage_poll`; the fetch itself is
//! `service::account_usage` (security-reviewed, untouched here).

use fleet_core::events::EventBus;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::account_usage::{AccountUsageSnapshot, UsageCache};
use fleet_core::service::account_usage_poll;
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Deserialize)]
pub struct RefreshAccountUsageArgs {
    pub account_uuid: String,
}

/// Every known account's cached usage snapshot. Never fetches.
#[tauri::command]
pub fn list_account_usage(
    store: State<'_, Arc<Mutex<Store>>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
    account_usage_poll::list_account_usage(&store, &cache)
}

/// Fetch `account_uuid`'s usage now if the floor allows, else return the
/// current snapshot unchanged. `E_NOTFOUND` for an unknown account.
#[tauri::command]
pub async fn refresh_account_usage(
    args: RefreshAccountUsageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
    bus: State<'_, Arc<dyn EventBus>>,
) -> Result<AccountUsageSnapshot, IpcError> {
    account_usage_poll::refresh_account_usage(&args.account_uuid, &store, &*ssh, &cache, &**bus)
        .await
}
