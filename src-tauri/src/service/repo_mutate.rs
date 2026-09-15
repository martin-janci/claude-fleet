//! Mutating git operations on a session's worktree: checkout, branch
//! create/delete, stage/unstage/commit, and remote sync. Reuses the shared
//! plumbing in `service::repo`. Branch names go through `validate::git_ref`,
//! hashes through `validate::commit_hash`, paths through
//! `validate::repo_rel_path`; every interpolated value is shell-quoted.
//!
//! Shared by the Tauri commands (`commands/mutate.rs`) and reachable from
//! the MCP layer.

use crate::ipc_error::{codes, IpcError};
use crate::service::repo::{ensure_clean, run_git, session_target, SessionIdArgs};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
pub struct CheckoutArgs {
    pub session_id: i64,
    pub branch: String,
}

/// Checkout a branch. Refuses (E_DIRTY) when the worktree has uncommitted
/// changes — the agent may be mid-edit. Never `--force`.
pub async fn repo_checkout(
    args: CheckoutArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.branch)?;
    let (host, name) = session_target(store, args.session_id)?;
    ensure_clean(ssh, &host, &name).await?;
    let body = format!("git -C \"$root\" checkout {}", quote(&args.branch));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct CheckoutCommitArgs {
    pub session_id: i64,
    pub hash: String,
}

/// Checkout a commit (detached HEAD). Same dirty guard as `repo_checkout`.
pub async fn repo_checkout_commit(
    args: CheckoutCommitArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    let (host, name) = session_target(store, args.session_id)?;
    ensure_clean(ssh, &host, &name).await?;
    let body = format!("git -C \"$root\" checkout {}", quote(&args.hash));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateBranchArgs {
    pub session_id: i64,
    pub name: String,
    pub start_point: Option<String>,
    pub checkout: bool,
}

/// Create a branch from HEAD or a start point (branch name or commit hash),
/// optionally checking it out.
pub async fn repo_create_branch(
    args: CreateBranchArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    // A start point may be a ref or a hash — accept either, validated.
    if let Some(sp) = &args.start_point {
        if crate::validate::commit_hash(sp).is_err() {
            crate::validate::git_ref(sp)?;
        }
    }
    let (host, name) = session_target(store, args.session_id)?;
    let sp = args
        .start_point
        .as_ref()
        .map(|s| format!(" {}", quote(s)))
        .unwrap_or_default();
    let verb = if args.checkout {
        "checkout -b"
    } else {
        "branch"
    };
    let body = format!("git -C \"$root\" {verb} {}{sp}", quote(&args.name));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct DeleteBranchArgs {
    pub session_id: i64,
    pub name: String,
    pub force: bool,
}

/// Delete a local branch (`-d`, or `-D` when `force`).
pub async fn repo_delete_branch(
    args: DeleteBranchArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    let (host, name) = session_target(store, args.session_id)?;
    let flag = if args.force { "-D" } else { "-d" };
    let body = format!("git -C \"$root\" branch {flag} {}", quote(&args.name));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct StageArgs {
    pub session_id: i64,
    pub paths: Vec<String>,
}

/// Stage the given worktree paths (`git add --`).
pub async fn repo_stage(
    args: StageArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| quote(p)).collect();
    let body = format!("git -C \"$root\" add -- {}", quoted.join(" "));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

/// Unstage the given paths (`git restore --staged --`).
pub async fn repo_unstage(
    args: StageArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| quote(p)).collect();
    let body = format!("git -C \"$root\" restore --staged -- {}", quoted.join(" "));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct CommitCreateArgs {
    pub session_id: i64,
    pub message: String,
    pub amend: bool,
}

/// Commit the staged changes. An empty message is rejected.
pub async fn repo_commit_create(
    args: CommitCreateArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    if args.message.trim().is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "commit message must not be empty",
        ));
    }
    let (host, name) = session_target(store, args.session_id)?;
    let amend = if args.amend { " --amend" } else { "" };
    let body = format!("git -C \"$root\" commit{amend} -m {}", quote(&args.message));
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}

/// `git fetch` (all remotes).
pub async fn repo_fetch(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    run_git(ssh, &host, &name, "git -C \"$root\" fetch --all --prune").await?;
    Ok(())
}

/// `git pull --ff-only` (refuse to create a merge commit silently).
pub async fn repo_pull(
    args: SessionIdArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    run_git(ssh, &host, &name, "git -C \"$root\" pull --ff-only").await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct PushArgs {
    pub session_id: i64,
    pub set_upstream: bool,
}

/// `git push`; with `set_upstream`, push the current branch and set upstream.
pub async fn repo_push(
    args: PushArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(store, args.session_id)?;
    let body = if args.set_upstream {
        "b=\"$(git -C \"$root\" rev-parse --abbrev-ref HEAD)\"; git -C \"$root\" push -u origin \"$b\""
            .to_string()
    } else {
        "git -C \"$root\" push".to_string()
    };
    run_git(ssh, &host, &name, &body).await?;
    Ok(())
}
