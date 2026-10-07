//! Tauri IPC wrappers for worktree management. Real logic lives in
//! `service::worktrees`.
//!
//! Remote mode: `list_host_worktrees` routes to its tool. It is the scan
//! itself, run by whichever side has the SSH route — this app when it owns
//! the fleet, the hub when it does not.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::worktrees::{self, HostWorktrees, ListHostWorktreesArgs};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

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
    routed::list_host_worktrees(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    /// The tool's parameters are this command's argument struct field for
    /// field — both required, neither defaulted — so `route` sends the
    /// struct itself rather than a `json!` literal.
    pub async fn list_host_worktrees(
        backend: &FleetBackend,
        args: ListHostWorktreesArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<HostWorktrees, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("list_host_worktrees", &args).await,
            None => worktrees::list_host_worktrees(args, store, ssh).await,
        }
    }
}
