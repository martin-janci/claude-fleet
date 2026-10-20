//! Autofix findings (`fleet_core::service::autofix`): the panel's "Problems
//! & fixes", the `fix` chat card and Control's own finding read and decide
//! them here. Paired, every command routes to the hub's `autofix` tool,
//! whose findings are the fleet's; standalone, this app is the hub and its
//! desktop is the person's own device, so it may apply and dismiss.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::service::autofix::api::{self, FindingView, Findings};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// The live findings, the ones fixed in the last day, and whether this
/// device may apply them.
#[tauri::command]
pub async fn autofix_findings(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Findings, IpcError> {
    routed::autofix_findings(&backend, &store).await
}

/// Check now: re-read every host's MCP entry, re-run the detectors, and
/// return the list as `autofix_findings` does. Applies nothing.
#[tauri::command]
pub async fn autofix_scan(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Findings, IpcError> {
    routed::autofix_scan(&backend, &store, &ssh).await
}

/// Fix: run one finding's fix and re-check; the row as it now stands.
#[tauri::command]
pub async fn autofix_apply(
    id: i64,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FindingView, IpcError> {
    routed::autofix_apply(&backend, &store, &ssh, id).await
}

/// Later: hide a finding until `until` (default a day).
#[tauri::command]
pub async fn autofix_dismiss(
    id: i64,
    until: Option<i64>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<FindingView, IpcError> {
    routed::autofix_dismiss(&backend, &store, id, until).await
}

pub mod routed {
    use super::*;
    use fleet_core::service::autofix::apply::{apply_one, live_agents, Actor, SshFixExec};
    use serde_json::json;

    pub async fn autofix_findings(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Findings, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("autofix_findings", &json!({ "action": "list" }))
                    .await
            }
            None => api::list(&*lock(store)?, None, true, fleet_core::store::now_unix()),
        }
    }

    pub async fn autofix_scan(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Findings, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("autofix_scan", &json!({ "action": "scan" }))
                    .await
            }
            None => {
                let now = fleet_core::store::now_unix();
                fleet_core::service::autofix::tick::scan_now(store, ssh, now).await?;
                api::list(&*lock(store)?, None, true, now)
            }
        }
    }

    pub async fn autofix_apply(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        id: i64,
    ) -> Result<FindingView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("autofix_apply", &json!({ "action": "apply", "id": id }))
                    .await
            }
            None => {
                let exec = SshFixExec { store, ssh };
                let row = apply_one(
                    store,
                    &exec,
                    id,
                    &Actor::desktop(),
                    &live_agents(ssh),
                    fleet_core::store::now_unix(),
                )
                .await?;
                Ok(FindingView::from(&row))
            }
        }
    }

    pub async fn autofix_dismiss(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        id: i64,
        until: Option<i64>,
    ) -> Result<FindingView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "autofix_dismiss",
                    &json!({ "action": "dismiss", "id": id, "until": until }),
                )
                .await
            }
            None => api::dismiss(&*lock(store)?, id, until, fleet_core::store::now_unix()),
        }
    }
}
