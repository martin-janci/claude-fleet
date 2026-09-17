//! Tauri commands for the mutating git operations of the Files tab. Thin
//! wrappers over `service::repo_mutate`.

use fleet_core::ipc_error::IpcError;
use fleet_core::service::repo::SessionIdArgs;
use fleet_core::service::repo_mutate::{
    self, CheckoutArgs, CheckoutCommitArgs, CommitCreateArgs, CreateBranchArgs, DeleteBranchArgs,
    PushArgs, StageArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Checkout a branch. Refuses (E_DIRTY) when the worktree has uncommitted
/// changes — the agent may be mid-edit. Never `--force`.
#[tauri::command]
pub async fn repo_checkout(
    args: CheckoutArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_checkout(args, &store, &ssh).await
}

/// Checkout a commit (detached HEAD). Same dirty guard as `repo_checkout`.
#[tauri::command]
pub async fn repo_checkout_commit(
    args: CheckoutCommitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_checkout_commit(args, &store, &ssh).await
}

/// Create a branch from HEAD or a start point (branch name or commit hash),
/// optionally checking it out.
#[tauri::command]
pub async fn repo_create_branch(
    args: CreateBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_create_branch(args, &store, &ssh).await
}

/// Delete a local branch (`-d`, or `-D` when `force`).
#[tauri::command]
pub async fn repo_delete_branch(
    args: DeleteBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_delete_branch(args, &store, &ssh).await
}

/// Stage the given worktree paths (`git add --`).
#[tauri::command]
pub async fn repo_stage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_stage(args, &store, &ssh).await
}

/// Unstage the given paths (`git restore --staged --`).
#[tauri::command]
pub async fn repo_unstage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_unstage(args, &store, &ssh).await
}

/// Commit the staged changes. An empty message is rejected.
#[tauri::command]
pub async fn repo_commit_create(
    args: CommitCreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_commit_create(args, &store, &ssh).await
}

/// `git fetch` (all remotes).
#[tauri::command]
pub async fn repo_fetch(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_fetch(args, &store, &ssh).await
}

/// `git pull --ff-only` (refuse to create a merge commit silently).
#[tauri::command]
pub async fn repo_pull(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_pull(args, &store, &ssh).await
}

/// `git push`; with `set_upstream`, push the current branch and set upstream.
#[tauri::command]
pub async fn repo_push(
    args: PushArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    repo_mutate::repo_push(args, &store, &ssh).await
}
