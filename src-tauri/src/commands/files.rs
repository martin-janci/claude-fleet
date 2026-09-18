//! Tauri commands for the Files & Diff viewer (iter 5). Thin wrappers over
//! `service::repo_read`.

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
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<ChangedFile>, IpcError> {
    repo_read::repo_changes(args, &store, &ssh).await
}

/// Flat worktree listing (tracked + untracked, gitignore respected).
#[tauri::command]
pub async fn repo_tree(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepoTree, IpcError> {
    repo_read::repo_tree(args, &store, &ssh).await
}

/// Read one worktree file's content (capped at `MAX_FILE_BYTES`).
#[tauri::command]
pub async fn repo_file(
    args: RepoFileArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileContent, IpcError> {
    repo_read::repo_file(args, &store, &ssh).await
}

/// Unified diff for one worktree file. Tracked changes diff against `HEAD`;
/// an untracked file falls back to `git diff --no-index` against `/dev/null`
/// so it still renders as an all-added diff.
#[tauri::command]
pub async fn repo_diff(
    args: RepoFileArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    repo_read::repo_diff(args, &store, &ssh).await
}
