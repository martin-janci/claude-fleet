//! Tauri IPC wrapper for `move_session` (the "Move to host…" action).
//! Logic lives in `service::move_session`.

use crate::ipc_error::IpcError;
use crate::service::move_session::{self, MoveReport, MoveSessionArgs};
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn move_session(
    args: MoveSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<MoveReport, IpcError> {
    move_session::move_session(args, &store, &ssh).await
}
