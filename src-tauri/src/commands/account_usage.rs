//! Tauri IPC wrappers for account usage (Task 4). Logic lives in
//! `service::account_usage_poll`; the fetch itself is
//! `service::account_usage` (security-reviewed, untouched here).
//!
//! Both refuse in remote mode. The cache they read is filled by
//! `spawn_account_usage_tick`, which a hub client does not start (Task 1), so
//! the local answer would be a permanently empty list presented as fact — and
//! the refresh path SSHes to the host from here. The hub's `usage_report`
//! tool answers per-session usage, a different shape from
//! `AccountUsageSnapshot`, so there is nothing to route to.

use crate::backend::FleetBackend;
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
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
    backend.refuse_local_only("list_account_usage")?;
    account_usage_poll::list_account_usage(&store, &cache)
}

/// Fetch `account_uuid`'s usage now if the floor allows, else return the
/// current snapshot unchanged. `E_NOTFOUND` for an unknown account.
#[tauri::command]
pub async fn refresh_account_usage(
    args: RefreshAccountUsageArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
    bus: State<'_, Arc<dyn EventBus>>,
) -> Result<AccountUsageSnapshot, IpcError> {
    backend.refuse_local_only("refresh_account_usage")?;
    account_usage_poll::refresh_account_usage(&args.account_uuid, &store, &*ssh, &cache, &**bus)
        .await
}
