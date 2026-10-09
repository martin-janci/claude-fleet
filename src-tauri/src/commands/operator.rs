//! Tauri IPC wrappers for the UX agent's lifecycle. Logic lives in
//! `service::operator`; this file only adapts `tauri::State` to plain
//! references.
//!
//! Both ROUTE in remote mode. That is not incidental: the agent panel is the
//! same panel on a hub-backed desktop, and the phone slice inherits these
//! tools unchanged. A `LocalOnly` verdict here would have closed that door.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::decide::control_route::ControlRoute;
use fleet_core::service::operator::{self, OperatorStatus};
use fleet_core::ssh::SshClient;
use fleet_core::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn ensure_operator(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    routed::ensure_operator(&backend, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn operator_status(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OperatorStatus, IpcError> {
    routed::operator_status(&backend, &store).await
}

/// Where a message just sent in Control goes (redesign step 9.9, Jev K2):
/// the receipt under the message. `none` when the feature is off.
#[tauri::command]
pub async fn control_route_propose(
    text: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ControlRoute, IpcError> {
    routed::control_route_propose(&backend, &store, text).await
}

/// The person kept or changed a Control receipt: records the follow-up.
#[tauri::command]
pub async fn control_route_follow(
    run_id: i64,
    chosen: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    routed::control_route_follow(&backend, &store, run_id, chosen).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::service::decide::{control_route, DecideCtx};
    use serde_json::json;

    pub async fn control_route_propose(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        text: String,
    ) -> Result<ControlRoute, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let args = json!({ "action": "propose", "text": text });
                // A hub from before K2 has no such tool: show nothing.
                match hub.route("control_route_propose", &args).await {
                    Err(e) if e.code == fleet_core::ipc_error::codes::E_HUB_PROTOCOL => {
                        Ok(ControlRoute::default())
                    }
                    r => r,
                }
            }
            None => {
                let ctx = DecideCtx::jev(Arc::clone(store));
                let scope = fleet_core::service::view_scope::ViewScope::internal();
                Ok(control_route::propose(&ctx, &scope, &text).await)
            }
        }
    }

    pub async fn control_route_follow(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        run_id: i64,
        chosen: String,
    ) -> Result<bool, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let args = json!({ "action": "follow", "run_id": run_id, "chosen": chosen });
                hub.route("control_route_follow", &args).await
            }
            None => {
                let s = fleet_core::ipc_error::lock(store)?;
                control_route::follow(&s, run_id, &chosen, fleet_core::store::now_unix())
            }
        }
    }

    pub async fn ensure_operator(
        backend: &FleetBackend,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.ensure_operator().await,
            None => operator::ensure_operator(store, ssh, reg).await,
        }
    }

    pub async fn operator_status(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<OperatorStatus, IpcError> {
        match backend.hub() {
            Some(hub) => hub.operator_status().await,
            None => operator::operator_status(store),
        }
    }
}
