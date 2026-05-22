//! Tauri IPC wrappers for project discovery. Logic lives in `service::projects`;
//! this file only adapts `tauri::State` to plain references.

use crate::ipc_error::IpcError;
use crate::service::projects::{self, ProjectTreeRow};
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub fn list_projects(store: State<'_, Arc<Mutex<Store>>>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    projects::list_projects(&store)
}

#[tauri::command]
pub async fn refresh_projects(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ProjectTreeRow>, IpcError> {
    projects::refresh_projects(&store).await
}

#[tauri::command]
pub async fn recreate_worktree(
    args: crate::service::sessions::RecreateWorktreeArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<crate::ssh::SshClient>>,
) -> Result<crate::store::WorktreeRow, IpcError> {
    crate::service::sessions::recreate_worktree(args, &store, &ssh).await
}
