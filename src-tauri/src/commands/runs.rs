//! The Automation screen's Runs list (Orbit Fleet redesign 8.3; the screen
//! is 8.4): every run on the fleet's behalf, newest first, each linked to
//! its sessions. Standalone it reads `service::runs` through the hub's own
//! reader (the desktop IS the local operator); paired, it routes to the
//! hub's `runs` tool, which cuts the list to this device's person scope.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::runs::{self, RunsArgs, RunsPage};
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// One page of runs and how many match in all.
#[tauri::command]
pub async fn list_runs(
    args: Option<RunsArgs>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RunsPage, IpcError> {
    routed::list_runs(&backend, &store, args.unwrap_or_default()).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::ipc_error::codes;
    use fleet_core::service::view_scope::ViewScope;

    /// The tool's arguments: `{action: "list"}` and the filters.
    pub fn tool_args(args: &RunsArgs) -> Result<serde_json::Value, IpcError> {
        let mut v = serde_json::to_value(args)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("list_runs: {e}")))?;
        if let Some(m) = v.as_object_mut() {
            m.insert("action".into(), "list".into());
        }
        Ok(v)
    }

    pub async fn list_runs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        args: RunsArgs,
    ) -> Result<RunsPage, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_runs", &tool_args(&args)?).await,
            None => runs::list(store, &ViewScope::internal(), &args),
        }
    }
}
