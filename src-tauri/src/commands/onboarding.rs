//! Tauri IPC wrappers for the first-run onboarding checklist. Read-only; not
//! exposed as MCP tools (so control-api-reference.md needs no regeneration).
//!
//! Both refuse in remote mode: each answers from the local `Store`'s host
//! rows, which are not the hub's fleet, and the checklist they feed is about
//! setting up a fleet this app would manage itself.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::hosts;
use fleet_core::service::onboarding::{self, LocalPrereqs, TunnelStatusRow};
use fleet_core::service::tunnel::TunnelSupervisor;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn check_local_prereqs(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LocalPrereqs, IpcError> {
    backend.local_only(
        "check_local_prereqs",
        "the onboarding checklist is about running a fleet from this machine, \
         which the hub is doing instead",
    )?;
    Ok(onboarding::local_prereqs(&store).await)
}

#[tauri::command]
pub fn tunnel_status(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
) -> Result<Vec<TunnelStatusRow>, IpcError> {
    backend.local_only(
        "tunnel_status",
        "the tunnels belong to the process that owns the fleet; check them on \
         the hub",
    )?;
    let hosts = hosts::list_hosts(&store)?;
    let alive = tunnels.snapshot();
    Ok(onboarding::map_tunnel_states(&hosts, &alive))
}
