//! Tauri IPC wrapper for `resolve_move` (Finish / Undo on the Transfer
//! sheet's partial view). Logic lives in `service::move_session::resolve`.
//!
//! Routes in remote mode: `ResolveMoveArgs` is exactly the tool's parameter
//! set (`session_id`, `action`), and the tool answers the same
//! `ResolveMoveReport`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::move_session::resolve::{self, ResolveMoveArgs, ResolveMoveReport};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn resolve_move(
    args: ResolveMoveArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ResolveMoveReport, IpcError> {
    routed::resolve_move(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn resolve_move(
        backend: &FleetBackend,
        args: ResolveMoveArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ResolveMoveReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("resolve_move", &args).await,
            None => resolve::resolve_move(args, store, ssh).await,
        }
    }
}
