//! Pinned dashboards (persona design step 6). Standalone it runs
//! `service::dashboards` here, as fleet's own rows; paired, the hub's
//! `dashboards` tool, where the person's other devices read them. A hub
//! that does not serve the tool yet keeps them in this desktop's own store,
//! as before.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::dashboards::{self, DashboardsArgs};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn dashboards(
    args: DashboardsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::dashboards(&backend, &store, args).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn dashboards(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: DashboardsArgs,
    ) -> Result<serde_json::Value, IpcError> {
        let local = |args: &DashboardsArgs| {
            dashboards::run(store, &ViewScope::internal(), args, None, true)
        };
        match backend.hub() {
            Some(hub) => match hub.route("dashboards", &args).await {
                // A hub without the tool: this desktop keeps them, as it
                // did before they moved to the hub.
                Err(e) if e.code == fleet_core::ipc_error::codes::E_HUB_PROTOCOL => local(&args),
                r => r,
            },
            None => local(&args),
        }
    }
}
