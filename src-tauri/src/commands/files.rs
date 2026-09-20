//! Tauri commands for the Files & Diff viewer (iter 5). Thin wrappers over
//! `service::repo_read`.
//!
//! All four have hub tools of the same name and route there in remote mode;
//! see `commands/history.rs` for the note on their wire types.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::repo::SessionIdArgs;
use fleet_core::service::repo_read::{
    self, ChangedFile, FileContent, FileDiff, RepoFileArgs, RepoTree,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// `git status` for a session's worktree.
#[tauri::command]
pub async fn repo_changes(
    args: SessionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<ChangedFile>, IpcError> {
    routed::repo_changes(&backend, args, &store, &ssh).await
}

/// Flat worktree listing (tracked + untracked, gitignore respected).
#[tauri::command]
pub async fn repo_tree(
    args: SessionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepoTree, IpcError> {
    routed::repo_tree(&backend, args, &store, &ssh).await
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`).
#[tauri::command]
pub async fn repo_file(
    args: RepoFileArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileContent, IpcError> {
    routed::repo_file(&backend, args, &store, &ssh).await
}

/// Unified diff for one worktree file. Tracked changes diff against `HEAD`;
/// an untracked file falls back to `git diff --no-index` against `/dev/null`
/// so it still renders as an all-added diff.
#[tauri::command]
pub async fn repo_diff(
    args: RepoFileArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    routed::repo_diff(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn repo_changes(
        backend: &FleetBackend,
        args: SessionIdArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<ChangedFile>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_changes", &args).await,
            None => repo_read::repo_changes(args, store, ssh).await,
        }
    }

    pub async fn repo_tree(
        backend: &FleetBackend,
        args: SessionIdArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<RepoTree, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_tree", &args).await,
            None => repo_read::repo_tree(args, store, ssh).await,
        }
    }

    pub async fn repo_file(
        backend: &FleetBackend,
        args: RepoFileArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<FileContent, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_file", &args).await,
            None => repo_read::repo_file(args, store, ssh).await,
        }
    }

    pub async fn repo_diff(
        backend: &FleetBackend,
        args: RepoFileArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<FileDiff, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_diff", &args).await,
            None => repo_read::repo_diff(args, store, ssh).await,
        }
    }
}
