//! Tauri IPC wrappers for worktree management. Real logic lives in
//! `service::worktrees`.

use crate::ipc_error::IpcError;
use crate::service::worktrees::{self, DeleteWorktreeArgs, ListWorktreesArgs, WorktreeOccupancy};
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn list_worktrees(
    args: ListWorktreesArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorktreeOccupancy>, IpcError> {
    worktrees::list_worktrees(args, &store)
}

#[tauri::command]
pub async fn delete_worktree(
    args: DeleteWorktreeArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    worktrees::delete_worktree(args, &store, &ssh).await
}
