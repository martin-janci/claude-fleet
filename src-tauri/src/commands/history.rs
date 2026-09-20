//! Tauri commands for the History & Branches views: commit log, branch list,
//! one commit's metadata + changed files, and a file's diff within a commit.
//! Thin wrappers over `service::repo_read`.
//!
//! All four have hub tools of the same name and route there in remote mode.
//! Their return types gained `Deserialize` in Task 3; because
//! `mcp::tools::repo` answers through plain `ok_json` rather than the
//! null-stripping encoder, they carry no `#[serde(default)]` and a renamed
//! field still fails loudly.

use crate::backend::FleetBackend;
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
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Commit>, IpcError> {
    routed::repo_log(&backend, args, &store, &ssh).await
}

/// Local + remote branches for a session's worktree.
#[tauri::command]
pub async fn repo_branches(
    args: SessionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<Branch>, IpcError> {
    routed::repo_branches(&backend, args, &store, &ssh).await
}

/// One commit's metadata + the files it changed (first-parent for merges).
#[tauri::command]
pub async fn repo_commit(
    args: RepoCommitArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<CommitDetail, IpcError> {
    routed::repo_commit(&backend, args, &store, &ssh).await
}

/// A single file's diff *within* a commit (first-parent for merges), so the
/// existing DiffView can render it.
#[tauri::command]
pub async fn repo_commit_diff(
    args: RepoCommitDiffArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FileDiff, IpcError> {
    routed::repo_commit_diff(&backend, args, &store, &ssh).await
}

pub(crate) mod routed {
    use super::*;

    /// The whole `RepoLogArgs` goes over, zeroes included: the desktop's
    /// `all`/`limit`/`skip` are concrete where the tool's are optional, and
    /// its own defaults (`all: true`, `limit: 50`) differ from this view's.
    /// Omitting a field the user left at its default would quietly change
    /// what the History view shows.
    pub async fn repo_log(
        backend: &FleetBackend,
        args: RepoLogArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<Commit>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_log", &args).await,
            None => repo_read::repo_log(args, store, ssh).await,
        }
    }

    pub async fn repo_branches(
        backend: &FleetBackend,
        args: SessionIdArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<Branch>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_branches", &args).await,
            None => repo_read::repo_branches(args, store, ssh).await,
        }
    }

    pub async fn repo_commit(
        backend: &FleetBackend,
        args: RepoCommitArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<CommitDetail, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_commit", &args).await,
            None => repo_read::repo_commit(args, store, ssh).await,
        }
    }

    pub async fn repo_commit_diff(
        backend: &FleetBackend,
        args: RepoCommitDiffArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<FileDiff, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("repo_commit_diff", &args).await,
            None => repo_read::repo_commit_diff(args, store, ssh).await,
        }
    }
}
