//! Tauri IPC wrappers for the first-run onboarding checklist. Read-only; not
//! exposed as MCP tools (so control-api-reference.md needs no regeneration).

use fleet_core::ipc_error::IpcError;
use fleet_core::service::hosts;
use fleet_core::service::onboarding::{self, LocalPrereqs, TunnelStatusRow};
use fleet_core::service::tunnel::TunnelSupervisor;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn check_local_prereqs(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LocalPrereqs, IpcError> {
    Ok(onboarding::local_prereqs(&store).await)
}

#[tauri::command]
pub fn tunnel_status(
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
) -> Result<Vec<TunnelStatusRow>, IpcError> {
    let hosts = hosts::list_hosts(&store)?;
    let alive = tunnels.snapshot();
    Ok(onboarding::map_tunnel_states(&hosts, &alive))
}
