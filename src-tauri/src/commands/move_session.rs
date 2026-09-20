//! Tauri IPC wrapper for `move_session` (the "Move to host…" action).
//! Logic lives in `service::move_session`.
//!
//! Routes in remote mode: `MoveSessionArgs` is exactly the tool's parameter
//! set (`session_id`, `target_host_alias`, `keep_source`, `strict`), and the tool
//! answers the same `MoveReport`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::move_session::{self, MoveReport, MoveSessionArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn move_session(
    args: MoveSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<MoveReport, IpcError> {
    routed::move_session(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn move_session(
        backend: &FleetBackend,
        args: MoveSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<MoveReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("move_session", &args).await,
            None => move_session::move_session(args, store, ssh).await,
        }
    }
}
