//! Tauri IPC wrappers for the first-run onboarding checklist. Read-only; not
//! exposed as MCP tools (so control-api-reference.md needs no regeneration).

use crate::ipc_error::IpcError;
use crate::service::hosts;
use crate::service::onboarding::{self, LocalPrereqs, TunnelStatusRow};
use crate::service::tunnel::TunnelSupervisor;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn check_local_prereqs() -> Result<LocalPrereqs, IpcError> {
    Ok(onboarding::local_prereqs().await)
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
