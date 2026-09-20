//! Tauri IPC wrappers for worktree management. Real logic lives in
//! `service::worktrees`.
//!
//! Remote mode: `list_worktrees` and `delete_worktree` route to their tools.
//! `list_host_worktrees` has none — it scans a host over SSH from here.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::worktrees::{
    self, DeleteWorktreeArgs, HostWorktrees, ListHostWorktreesArgs, ListWorktreesArgs,
    WorktreeOccupancy,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn list_worktrees(
    args: ListWorktreesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorktreeOccupancy>, IpcError> {
    routed::list_worktrees(&backend, args, &store).await
}

/// The worktrees of one project as they exist on one host (a remote host
/// is scanned over SSH and cached as its rows). Feeds the New-session
/// dialog's worktree picker.
#[tauri::command]
pub async fn list_host_worktrees(
    args: ListHostWorktreesArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostWorktrees, IpcError> {
    backend.refuse_local_only("list_host_worktrees")?;
    worktrees::list_host_worktrees(args, &store, &ssh).await
}

#[tauri::command]
pub async fn delete_worktree(
    args: DeleteWorktreeArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    routed::delete_worktree(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn list_worktrees(
        backend: &FleetBackend,
        args: ListWorktreesArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorktreeOccupancy>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_worktrees", &args).await,
            None => worktrees::list_worktrees(args, store),
        }
    }

    pub async fn delete_worktree(
        backend: &FleetBackend,
        args: DeleteWorktreeArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<(), IpcError> {
        match backend.hub() {
            // The tool answers prose, so the text is read — which is what
            // surfaces a tool error — and discarded.
            Some(hub) => hub.route_text("delete_worktree", &args).await.map(|_| ()),
            None => worktrees::delete_worktree(args, store, ssh).await,
        }
    }
}
