//! Mutating git commands for the Files tab: checkout, branch create/delete,
//! stage/commit, and remote sync. Reuses the shared plumbing in `repo.rs`.
//! Branch names go through `validate::git_ref`, hashes through
//! `validate::commit_hash`, paths through `validate::repo_rel_path`; every
//! interpolated value is shell-quoted.

use crate::commands::repo::{repo_err, repo_script, run_in_repo, session_target};
use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::State;

/// True when `git status --porcelain` output indicates a dirty worktree.
fn is_dirty(porcelain: &[u8]) -> bool {
    !String::from_utf8_lossy(porcelain).trim().is_empty()
}

#[derive(Deserialize)]
pub struct CheckoutArgs {
    pub session_id: i64,
    pub branch: String,
}

/// Checkout a branch. Refuses (E_DIRTY) when the worktree has uncommitted
/// changes — the agent may be mid-edit. Never `--force`.
#[tauri::command]
pub async fn repo_checkout(
    args: CheckoutArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.branch)?;
    let (host, name) = session_target(&store, args.session_id)?;
    // Guard: check dirty first.
    let status = repo_script(&name, "git -C \"$root\" status --porcelain");
    let so = run_in_repo(&ssh, &host, &status).await?;
    if !so.status.success() {
        return Err(repo_err(&so));
    }
    if is_dirty(&so.stdout) {
        return Err(IpcError::new(
            "E_DIRTY",
            "worktree has uncommitted changes — the agent may have work in progress",
        ));
    }
    let body = format!("git -C \"$root\" checkout {}", quote(&args.branch));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CheckoutCommitArgs {
    pub session_id: i64,
    pub hash: String,
}

/// Checkout a commit (detached HEAD). Same dirty guard as `repo_checkout`.
#[tauri::command]
pub async fn repo_checkout_commit(
    args: CheckoutCommitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::commit_hash(&args.hash)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let status = repo_script(&name, "git -C \"$root\" status --porcelain");
    let so = run_in_repo(&ssh, &host, &status).await?;
    if !so.status.success() {
        return Err(repo_err(&so));
    }
    if is_dirty(&so.stdout) {
        return Err(IpcError::new(
            "E_DIRTY",
            "worktree has uncommitted changes — the agent may have work in progress",
        ));
    }
    let body = format!("git -C \"$root\" checkout {}", quote(&args.hash));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
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
#[tauri::command]
pub async fn repo_create_branch(
    args: CreateBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    // A start point may be a ref or a hash — accept either, validated.
    if let Some(sp) = &args.start_point {
        if crate::validate::commit_hash(sp).is_err() {
            crate::validate::git_ref(sp)?;
        }
    }
    let (host, name) = session_target(&store, args.session_id)?;
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
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct DeleteBranchArgs {
    pub session_id: i64,
    pub name: String,
    pub force: bool,
}

/// Delete a local branch (`-d`, or `-D` when `force`).
#[tauri::command]
pub async fn repo_delete_branch(
    args: DeleteBranchArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    crate::validate::git_ref(&args.name)?;
    let (host, name) = session_target(&store, args.session_id)?;
    let flag = if args.force { "-D" } else { "-d" };
    let body = format!("git -C \"$root\" branch {flag} {}", quote(&args.name));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct StageArgs {
    pub session_id: i64,
    pub paths: Vec<String>,
}

/// Stage the given worktree paths (`git add --`).
#[tauri::command]
pub async fn repo_stage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| quote(p)).collect();
    let body = format!("git -C \"$root\" add -- {}", quoted.join(" "));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

/// Unstage the given paths (`git restore --staged --`).
#[tauri::command]
pub async fn repo_unstage(
    args: StageArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    for p in &args.paths {
        crate::validate::repo_rel_path(p)?;
    }
    if args.paths.is_empty() {
        return Ok(());
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let quoted: Vec<String> = args.paths.iter().map(|p| quote(p)).collect();
    let body = format!("git -C \"$root\" restore --staged -- {}", quoted.join(" "));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CommitCreateArgs {
    pub session_id: i64,
    pub message: String,
    pub amend: bool,
}

/// Commit the staged changes. An empty message is rejected.
#[tauri::command]
pub async fn repo_commit_create(
    args: CommitCreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    if args.message.trim().is_empty() {
        return Err(IpcError::new(
            "E_INVALID",
            "commit message must not be empty",
        ));
    }
    let (host, name) = session_target(&store, args.session_id)?;
    let amend = if args.amend { " --amend" } else { "" };
    let body = format!("git -C \"$root\" commit{amend} -m {}", quote(&args.message));
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct SessionIdArgs {
    pub session_id: i64,
}

/// `git fetch` (all remotes).
#[tauri::command]
pub async fn repo_fetch(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let out = run_in_repo(
        &ssh,
        &host,
        &repo_script(&name, "git -C \"$root\" fetch --all --prune"),
    )
    .await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

/// `git pull --ff-only` (refuse to create a merge commit silently).
#[tauri::command]
pub async fn repo_pull(
    args: SessionIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let out = run_in_repo(
        &ssh,
        &host,
        &repo_script(&name, "git -C \"$root\" pull --ff-only"),
    )
    .await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct PushArgs {
    pub session_id: i64,
    pub set_upstream: bool,
}

/// `git push`; with `set_upstream`, push the current branch and set upstream.
#[tauri::command]
pub async fn repo_push(
    args: PushArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let (host, name) = session_target(&store, args.session_id)?;
    let body = if args.set_upstream {
        "b=\"$(git -C \"$root\" rev-parse --abbrev-ref HEAD)\"; git -C \"$root\" push -u origin \"$b\""
            .to_string()
    } else {
        "git -C \"$root\" push".to_string()
    };
    let out = run_in_repo(&ssh, &host, &repo_script(&name, &body)).await?;
    if !out.status.success() {
        return Err(repo_err(&out));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_dirty_detects_changes() {
        assert!(!is_dirty(b""));
        assert!(!is_dirty(b"   \n"));
        assert!(is_dirty(b" M src/x.rs\n"));
        assert!(is_dirty(b"?? new.txt\n"));
    }
}
