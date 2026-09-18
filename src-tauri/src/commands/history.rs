//! Tauri commands for the History & Branches views: commit log, branch list,
//! one commit's metadata + changed files, and a file's diff within a commit.
//! Thin wrappers over `service::repo_read`.

use fleet_core::ipc_error::IpcError;
use fleet_core::service::repo::SessionIdArgs;
use fleet_core::service::repo_read::{
    self, Branch, Commit, CommitDetail, FileDiff, RepoCommitArgs, RepoCommitDiffArgs, RepoLogArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Commit log for a session's worktree.
#[tauri::command]
pub async fn repo_log(
    args: RepoLogArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Commit>, IpcError> {
    repo_read::repo_log(args, &store, &ssh).await
}

/// Local + remote branches for a session's worktree.
#[tauri::command]
pub async fn repo_branches(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Branch>, IpcError> {
    repo_read::repo_branches(args, &store, &ssh).await
}

/// One commit's metadata + the files it changed (first-parent for merges).
#[tauri::command]
pub async fn repo_commit(
    args: RepoCommitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<CommitDetail, IpcError> {
    repo_read::repo_commit(args, &store, &ssh).await
}

/// A single file's diff *within* a commit (first-parent for merges), so the
/// existing DiffView can render it.
#[tauri::command]
pub async fn repo_commit_diff(
    args: RepoCommitDiffArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    repo_read::repo_commit_diff(args, &store, &ssh).await
}
