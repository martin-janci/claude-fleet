//! Wizards that resume on another device (gap plan G7.2). Standalone it
//! runs `service::wizard_state` here, as fleet's own rows; paired, the
//! hub's `wizard_state` tool, where the person's other devices read it.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::view_scope::ViewScope;
use fleet_core::service::wizard_state::{self, WizardStateArgs};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn wizard_state(
    args: WizardStateArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::wizard_state(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn wizard_state(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: WizardStateArgs,
    ) -> Result<serde_json::Value, IpcError> {
        match backend.hub() {
            Some(hub) => match hub.route("wizard_state", &args).await {
                // A hub before contract 17 has no such tool: nothing saved
                // there, so nothing to resume, and a save keeps nothing.
                Err(e) if e.code == fleet_core::ipc_error::codes::E_HUB_PROTOCOL => {
                    Ok(before_wizard_state(&args.action))
                }
                r => r,
            },
            None => wizard_state::run(store, &ViewScope::internal(), &args, None),
        }
    }

    /// What `wizard_state` answers on a hub that does not serve it: no rows,
    /// no row, nothing removed, and a save that kept nothing (`null`).
    pub fn before_wizard_state(action: &str) -> serde_json::Value {
        match action {
            "list" => serde_json::json!([]),
            "clear" => serde_json::json!({ "removed": false }),
            _ => serde_json::Value::Null,
        }
    }
}
