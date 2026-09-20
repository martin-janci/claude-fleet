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

pub(crate) mod routed {
    use super::*;

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
