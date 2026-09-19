//! Tauri commands for the mutating git operations of the Files tab. Thin
//! wrappers over `service::repo_mutate`.
//!
//! None of the ten has a hub tool: the control API exposes the repo *reads*
//! (`mcp::tools::repo`) and deliberately no writes, because a git mutation on
//! a session's worktree is the agent's business and a remote client staging
//! or committing under it would race whatever it is doing. So all ten refuse
//! in remote mode. They run over this machine's SSH connection, which a
//! hub-client desktop has no reason to have.

use crate::backend::FleetBackend;
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
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_checkout")?;
    repo_mutate::repo_checkout(args, &store, &ssh).await
}

/// Checkout a commit (detached HEAD). Same dirty guard as `repo_checkout`.
#[tauri::command]
pub async fn repo_checkout_commit(
    args: CheckoutCommitArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_checkout_commit")?;
    repo_mutate::repo_checkout_commit(args, &store, &ssh).await
}

/// Create a branch from HEAD or a start point (branch name or commit hash),
/// optionally checking it out.
#[tauri::command]
pub async fn repo_create_branch(
    args: CreateBranchArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_create_branch")?;
    repo_mutate::repo_create_branch(args, &store, &ssh).await
}

/// Delete a local branch (`-d`, or `-D` when `force`).
#[tauri::command]
pub async fn repo_delete_branch(
    args: DeleteBranchArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_delete_branch")?;
    repo_mutate::repo_delete_branch(args, &store, &ssh).await
}

/// Stage the given worktree paths (`git add --`).
#[tauri::command]
pub async fn repo_stage(
    args: StageArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_stage")?;
    repo_mutate::repo_stage(args, &store, &ssh).await
}

/// Unstage the given paths (`git restore --staged --`).
#[tauri::command]
pub async fn repo_unstage(
    args: StageArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_unstage")?;
    repo_mutate::repo_unstage(args, &store, &ssh).await
}

/// Commit the staged changes. An empty message is rejected.
#[tauri::command]
pub async fn repo_commit_create(
    args: CommitCreateArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_commit_create")?;
    repo_mutate::repo_commit_create(args, &store, &ssh).await
}

/// `git fetch` (all remotes).
#[tauri::command]
pub async fn repo_fetch(
    args: SessionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_fetch")?;
    repo_mutate::repo_fetch(args, &store, &ssh).await
}

/// `git pull --ff-only` (refuse to create a merge commit silently).
#[tauri::command]
pub async fn repo_pull(
    args: SessionIdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_pull")?;
    repo_mutate::repo_pull(args, &store, &ssh).await
}

/// `git push`; with `set_upstream`, push the current branch and set upstream.
#[tauri::command]
pub async fn repo_push(
    args: PushArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    backend.refuse_local_only("repo_push")?;
    repo_mutate::repo_push(args, &store, &ssh).await
}
