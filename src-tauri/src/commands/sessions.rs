//! Tauri IPC wrappers for tmux session management. Logic lives in
//! `service::sessions`; this file only adapts `tauri::State` to plain
//! references.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::service::sessions::{
    self, KillSessionArgs, NewSessionArgs, RelatedSessionsArgs, RenameSessionArgs,
    RestartSessionArgs, SendPromptArgs, SpawnReviewArgs,
};
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn list_sessions(
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    sessions::list_sessions(&store, &ssh).await
}

#[tauri::command]
pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SessionRow>, IpcError> {
    sessions::related_sessions(args, &store)
}

#[tauri::command]
pub async fn new_session(
    args: NewSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    sessions::new_session(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    sessions::kill_session(args, &store, &ssh).await
}

#[tauri::command]
pub async fn rename_session(
    args: RenameSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::rename_session(args, &store, &ssh).await
}

#[tauri::command]
pub async fn restart_session(
    args: RestartSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::restart_session(args, &store, &ssh).await
}

#[tauri::command]
pub async fn send_prompt(
    args: SendPromptArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    sessions::send_prompt(args, &ssh).await
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::spawn_review(args, &store, &ssh).await
}
