//! Mutating git operations on a session's worktree: checkout, branch
//! create/delete (one, or every merged one), stage/unstage/commit, and remote
//! sync. Reuses the shared
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
use serde::{Deserialize, Serialize};
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
pub struct DeleteMergedArgs {
    pub session_id: i64,
    /// Local branches the caller saw flagged `merged` and asked to delete.
    pub names: Vec<String>,
    /// Remote branches (`origin/feat`) the caller saw flagged `merged` and
    /// asked to delete on their remote (`git push <remote> --delete`).
    #[serde(default)]
    pub remotes: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct DeleteMergedResult {
    pub deleted: Vec<String>,
    /// Asked for but kept: no longer merged into the base, the base or its
    /// local twin, checked out here or in another worktree, or gone.
    pub kept: Vec<String>,
}

/// Delete the named local branches that the base branch (see
/// `repo_read::BASE_BRANCH_SH`) still contains. Each name is re-checked on
/// the host at delete time, so a branch that gained a commit since the list
/// was read is kept rather than lost; `-D` only because `-d` measures against
/// HEAD or the upstream, not the base. Refuses when there is no base branch.
///
/// `remotes` are deleted on their remote the same way (gap plan G7.10):
/// re-checked against the base after a fetch of that ref, never the base
/// itself, and kept when the push is refused.
pub async fn repo_delete_merged_branches(
    args: DeleteMergedArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<DeleteMergedResult, IpcError> {
    if args.names.is_empty() && args.remotes.is_empty() {
        return Ok(DeleteMergedResult::default());
    }
    for n in args.names.iter().chain(&args.remotes) {
        crate::validate::git_ref(n)?;
    }
    for r in &args.remotes {
        if !r.contains('/') {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{r:?} is not a remote branch (remote/branch)"),
            ));
        }
    }
    let (host, name) = session_target(store, args.session_id)?;
    let body = delete_merged_body(&args.names, &args.remotes);
    let out = run_git(ssh, &host, &name, &body).await?;
    parse_delete_merged(&out.stdout)
}

fn delete_merged_body(names: &[String], remotes: &[String]) -> String {
    let quoted = |v: &[String]| v.iter().map(|n| quote(n)).collect::<Vec<_>>().join(" ");
    format!(
        "{base}\
         if [ -z \"$base\" ]; then echo cf-no-base; exit 0; fi\n\
         for n in {names}; do\n\
           if [ \"$n\" != \"$base\" ] && [ \"$n\" != \"$base_local\" ] \
              && git -C \"$root\" merge-base --is-ancestor \"refs/heads/$n\" \"$base\" 2>/dev/null \
              && git -C \"$root\" branch -D \"$n\" >/dev/null 2>&1; then\n\
             printf 'D %s\\n' \"$n\"\n\
           else\n\
             printf 'K %s\\n' \"$n\"\n\
           fi\n\
         done\n\
         for n in {remotes}; do\n\
           r=\"${{n%%/*}}\"; b=\"${{n#*/}}\"\n\
           if [ \"$n\" != \"$base\" ] && [ \"$b\" != \"$base_local\" ] && [ \"$b\" != HEAD ] \
              && git -C \"$root\" remote | grep -qxF -- \"$r\" \
              && git -C \"$root\" fetch -q \"$r\" \"+refs/heads/$b:refs/remotes/$n\" >/dev/null 2>&1 \
              && git -C \"$root\" merge-base --is-ancestor \"refs/remotes/$n\" \"$base\" 2>/dev/null \
              && git -C \"$root\" push -q \"$r\" --delete \"$b\" >/dev/null 2>&1; then\n\
             printf 'D %s\\n' \"$n\"\n\
           else\n\
             printf 'K %s\\n' \"$n\"\n\
           fi\n\
         done",
        base = crate::service::repo_read::BASE_BRANCH_SH,
        names = quoted(names),
        remotes = quoted(remotes),
    )
}

fn parse_delete_merged(raw: &[u8]) -> Result<DeleteMergedResult, IpcError> {
    let text = String::from_utf8_lossy(raw);
    if text.lines().any(|l| l == "cf-no-base") {
        return Err(IpcError::new(
            codes::E_REPO,
            "no base branch (origin/HEAD, main or master) to tell merged branches by",
        ));
    }
    let mut r = DeleteMergedResult::default();
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("D ") {
            r.deleted.push(n.to_string());
        } else if let Some(n) = line.strip_prefix("K ") {
            r.kept.push(n.to_string());
        }
    }
    Ok(r)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::service::repo_read::git_fixture;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[cfg(unix)]
    fn local_branches(dir: &std::path::Path) -> Vec<String> {
        let out = git_fixture::run_body(
            dir,
            "git -C \"$root\" for-each-ref --format='%(refname:short)' refs/heads",
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn delete_merged_deletes_only_what_the_base_still_contains() {
        let tmp = tempfile::tempdir().unwrap();
        let work = git_fixture::branches_repo(tmp.path());
        let asked = names(&["done", "old", "open", "main", "fresh", "gone"]);
        let out = git_fixture::run_body(&work, &delete_merged_body(&asked, &[]));
        let r = parse_delete_merged(&out.stdout).unwrap();
        assert_eq!(r.deleted, names(&["done", "old"]));
        // open: not merged; main: the base's twin; fresh: checked out;
        // gone: never existed.
        assert_eq!(r.kept, names(&["open", "main", "fresh", "gone"]));
        assert_eq!(local_branches(&work), names(&["fresh", "main", "open"]));
    }

    #[cfg(unix)]
    #[test]
    fn delete_merged_refuses_without_a_base_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git_fixture::git(dir, &["init", "-q", "-b", "trunk"]);
        git_fixture::commit_file(dir, "a.txt", "a\n", "first");
        git_fixture::git(dir, &["branch", "side"]);
        let out = git_fixture::run_body(dir, &delete_merged_body(&names(&["side"]), &[]));
        let err = parse_delete_merged(&out.stdout).unwrap_err();
        assert_eq!(err.code, codes::E_REPO);
        assert_eq!(local_branches(dir), names(&["side", "trunk"]));
    }

    fn remote_branches(dir: &std::path::Path) -> Vec<String> {
        let out = git_fixture::run_body(
            dir,
            "git -C \"$root\" for-each-ref --format='%(refname:short)' refs/heads",
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn delete_merged_deletes_merged_remote_branches_on_their_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let work = git_fixture::branches_repo(tmp.path());
        git_fixture::git(&work, &["push", "-q", "origin", "open"]);
        let origin = tmp.path().join("origin.git");
        let asked = names(&[
            "origin/done",
            "origin/open",
            "origin/main",
            "nope/done",
            "origin/gone",
        ]);
        let out = git_fixture::run_body(&work, &delete_merged_body(&[], &asked));
        let r = parse_delete_merged(&out.stdout).unwrap();
        assert_eq!(r.deleted, names(&["origin/done"]));
        // open: not merged; main: the base; nope: no such remote; gone: no
        // such branch on the remote.
        assert_eq!(
            r.kept,
            names(&["origin/open", "origin/main", "nope/done", "origin/gone"])
        );
        assert_eq!(remote_branches(&origin), names(&["main", "open"]));
        // Local branches are left alone.
        assert!(local_branches(&work).contains(&"done".to_string()));
    }

    #[test]
    fn delete_merged_names_are_quoted_one_word_each() {
        let body = delete_merged_body(&names(&["feat/a$(x)"]), &names(&["origin/b$(y)"]));
        assert!(body.contains("'feat/a$(x)'"), "{body}");
        assert!(body.contains("'origin/b$(y)'"), "{body}");
    }
}
