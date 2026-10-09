//! Start rules on the desktop (redesign step 8.11): the start popover's
//! "Add rule PD-* → papaya-pos?" and the rules list. Standalone it runs
//! `service::start_rules` here; paired, the hub's `start_rules` tool, where
//! the hub's own starts are decided and tallied.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::start_rules::{self, StartRulesArgs};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn start_rules(
    args: StartRulesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::start_rules(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn start_rules(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: StartRulesArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("start_rules", &args).await,
            None => start_rules::run(store, &ViewScope::internal(), &args),
        }
    }
}
