//! Tauri IPC wrappers for worktree management. Real logic lives in
//! `service::worktrees`.

use crate::ipc_error::IpcError;
use crate::service::worktrees::{
    self, DeleteWorktreeArgs, HostWorktrees, ListHostWorktreesArgs, ListWorktreesArgs,
    WorktreeOccupancy,
};
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

/// The worktrees of one project as they exist on one host (a remote host
/// is scanned over SSH and cached as its rows). Feeds the New-session
/// dialog's worktree picker.
#[tauri::command]
pub async fn list_host_worktrees(
    args: ListHostWorktreesArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostWorktrees, IpcError> {
    worktrees::list_host_worktrees(args, &store, &ssh).await
}

#[tauri::command]
pub async fn delete_worktree(
    args: DeleteWorktreeArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    worktrees::delete_worktree(args, &store, &ssh).await
}
