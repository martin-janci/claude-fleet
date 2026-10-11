//! Tauri IPC wrappers for account usage. Logic lives in
//! `service::account_usage_poll`; the fetch itself is
//! `service::account_usage` (security-reviewed, untouched here).
//!
//! The cache they read is filled by `spawn_account_usage_tick`, which a hub
//! client does not start. So paired, `list_account_usage` routes to the
//! hub's `account_usage` tool (hub contract 11), which serves the hub's own
//! answers, and `check_account_headroom` to the hub's tool of that name
//! (contract 14); `refresh_account_usage` and the rest refuse, since a refresh
//! SSHes to the host from here and this app keeps no history while a hub
//! owns the fleet.

use crate::backend::FleetBackend;
use fleet_core::events::EventBus;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::account_limits::{self, CheckAccountHeadroomArgs, Headroom};
use fleet_core::service::account_spend::{self, AccountSpend};
use fleet_core::service::account_usage::{AccountUsageSnapshot, UsageCache};
use fleet_core::service::account_usage_poll;
use fleet_core::service::decide::host_placement::{
    self, ProposeHostArgs, RecordHostStartArgs, SuggestedHost,
};
use fleet_core::service::decide::DecideCtx;
use fleet_core::ssh::SshClient;
use fleet_core::store::{Store, UsageSnapshotRow};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Deserialize)]
pub struct RefreshAccountUsageArgs {
    pub account_uuid: String,
}

#[derive(Deserialize)]
pub struct AccountUsageHistoryArgs {
    pub account_uuid: String,
    /// Unix seconds; snapshots fetched before it are left out.
    pub since: i64,
}

#[derive(Deserialize)]
pub struct AccountSpendArgs {
    /// Unix seconds; spend on UTC days before its day is left out.
    pub since: i64,
    /// One account only; every account when absent.
    #[serde(default)]
    pub account_uuid: Option<String>,
}

/// Every known account's cached usage snapshot. Never fetches. Paired, the
/// hub's answers.
#[tauri::command]
pub async fn list_account_usage(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
    routed::list_account_usage(&backend, &store, &cache).await
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

/// One account's stored usage snapshots since `since`, oldest first (the
/// Accounts page's history). Never fetches. `E_NOTFOUND` for an unknown
/// account. Refused in remote mode like the two above: this app's store has
/// no history while a hub owns the fleet.
#[tauri::command(async)]
pub fn account_usage_history(
    args: AccountUsageHistoryArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<UsageSnapshotRow>, IpcError> {
    backend.refuse_local_only("account_usage_history")?;
    account_usage_poll::account_usage_history(&args.account_uuid, args.since, &store)
}

/// Whether starting under a login on a host crosses `accounts.pause_at`, and
/// the login on that host with the most headroom (redesign step 4.4). Reads
/// the cache only. Paired, the hub's `check_account_headroom` (contract 14),
/// which answers from the usage its bus followed.
#[tauri::command]
pub async fn check_account_headroom(
    args: CheckAccountHeadroomArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
) -> Result<Headroom, IpcError> {
    routed::check_account_headroom(&backend, args, &store, &cache).await
}

/// Live spend per account and per model since `since`, from the
/// `usage_daily_account` roll-up (redesign step 4.2). Refused in remote mode:
/// this app collects no usage while a hub owns the fleet.
#[tauri::command(async)]
pub fn account_spend(
    args: AccountSpendArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AccountSpend>, IpcError> {
    backend.refuse_local_only("account_spend")?;
    account_spend::account_spend(args.since, args.account_uuid.as_deref(), &store)
}

/// The decision model's host for a new session of a project (redesign step
/// 4.11, Jev N5 `host_placement`): `Some` only in `assist` when two or more
/// hosts are left after the limits and the offline ones. Off by default.
/// Paired, the hub's `propose_host_placement` answers (gap plan G7.3): the
/// hub owns the decision model and the usage.
#[tauri::command]
pub async fn propose_host_placement(
    args: ProposeHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    cache: State<'_, Arc<Mutex<UsageCache>>>,
) -> Result<Option<SuggestedHost>, IpcError> {
    routed::propose_host_placement(&backend, &store, &cache, args).await
}

/// After a person's start of a project landed on a host: marks the decision
/// model's host proposal confirmed or corrected (best effort, never fails a
/// start). Refused in remote mode like the proposal.
#[tauri::command(async)]
pub fn record_host_placement(
    args: RecordHostStartArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    backend.refuse_local_only("record_host_placement")?;
    let s = fleet_core::ipc_error::lock(&store)?;
    host_placement::record_start(
        &s,
        args.project_id,
        &args.host_alias,
        fleet_core::store::now_unix(),
    )
}

pub(crate) mod routed {
    use super::*;

    pub async fn propose_host_placement(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        cache: &Mutex<UsageCache>,
        args: ProposeHostArgs,
    ) -> Result<Option<SuggestedHost>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("propose_host_placement", &args).await,
            None => {
                let ctx = DecideCtx::jev(Arc::clone(store));
                host_placement::propose(&ctx, cache, &args).await
            }
        }
    }

    pub async fn list_account_usage(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        cache: &Mutex<UsageCache>,
    ) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_account_usage().await,
            None => account_usage_poll::list_account_usage(store, cache),
        }
    }

    pub async fn check_account_headroom(
        backend: &FleetBackend,
        args: CheckAccountHeadroomArgs,
        store: &Mutex<Store>,
        cache: &Mutex<UsageCache>,
    ) -> Result<Headroom, IpcError> {
        match backend.hub() {
            Some(hub) => hub.check_account_headroom(&args).await,
            None => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                account_limits::check_account_headroom(&args, store, cache, now)
            }
        }
    }
}
