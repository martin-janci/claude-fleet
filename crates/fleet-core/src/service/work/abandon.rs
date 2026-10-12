//! Cancel a start (task → session spec P-6): `work_link { action:
//! abandon_start, session_id }`, the desktop's `abandon_start`.
//!
//! A start that turns out wrong (the wrong host, the wrong task, a trust
//! prompt the person does not want to answer) should leave nothing behind.
//! This ends the session, removes the checkout the start made and deletes
//! the branch it made, but ONLY when nothing in them can be lost:
//!
//! - the session's newest `start_spawned` event says this start CREATED the
//!   session's checkout (a reused worktree is never removed);
//! - the checkout has no changes (`git status --porcelain` is empty);
//! - the checkout's HEAD carries no commit that no other branch or remote
//!   has.
//!
//! Anything else is `E_DIRTY`, with what was found in its details, and
//! nothing is touched. The order afterwards keeps a race safe: the session
//! is killed first, so Claude writes nothing after the check; `git worktree
//! remove` without `--force` still refuses a tree that changed meanwhile;
//! and `git branch -d` (not `-D`) refuses a branch that grew a commit.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::trackers::tickets::{StartSpawned, START_SPAWNED};
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The timeline kind a cancelled start leaves (best-effort: the row goes
/// with the kill).
pub const START_ABANDONED: &str = "start_abandoned";

/// At most this many changed paths are named in an `E_DIRTY`.
const DIRTY_NAMED_MAX: usize = 20;

const SEP: &str = "--fleet-abandon--";

/// What [`abandon_start`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbandonOutcome {
    pub session_id: i64,
    /// The checkout was removed.
    pub worktree_removed: bool,
    /// The start's branch was deleted. `false` leaves an empty branch
    /// behind (`git branch -d` refused it); nothing is lost either way.
    pub branch_deleted: bool,
}

/// Everything the cancel acts on, resolved under one store guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbandonPlan {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub worktree_id: i64,
    pub worktree_path: String,
    pub project_base: String,
    pub branch: String,
}

fn dirty(reason: &str, message: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_DIRTY, message).with_details(serde_json::json!({ "reason": reason }))
}

/// A branch name `git rev-list --exclude` takes literally: no glob
/// characters (a start's branch is a slug, so this refuses nothing real).
fn literal_branch(branch: &str) -> bool {
    !branch.is_empty() && !branch.contains(['*', '?', '[', '\\'])
}

/// PURE (over the store): what cancelling `session_id`'s start would act
/// on, or why it may not.
pub fn plan_abandon(s: &Store, session_id: i64) -> Result<AbandonPlan, IpcError> {
    let row = s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
    })?;
    crate::service::operator::refuse_if_operator(
        s,
        &row.host_alias,
        &row.tmux_name,
        "abandon_start",
    )?;
    let started: Option<StartSpawned> = s
        .newest_session_event_of(session_id, &[START_SPAWNED])?
        .and_then(|e| e.detail)
        .and_then(|d| serde_json::from_str(&d).ok());
    let Some(started) = started else {
        return Err(dirty(
            "not_a_start",
            format!(
                "{} was not made by a start; end it from the sidebar",
                row.tmux_name
            ),
        ));
    };
    let (Some(wid), true) = (row.worktree_id, started.new_worktree) else {
        return Err(dirty(
            "checkout_not_the_starts",
            "this start reused an existing checkout, which a cancel never removes",
        ));
    };
    if started.worktree_id != Some(wid) {
        return Err(dirty(
            "checkout_not_the_starts",
            "the session is no longer in the checkout its start made",
        ));
    }
    let branch = started.branch.unwrap_or_default();
    if !literal_branch(&branch) {
        return Err(dirty(
            "branch_unknown",
            "the start's branch is not known, so its commits cannot be checked",
        ));
    }
    let project_base = match row.project_id {
        Some(pid) => s.project_base_path(pid)?,
        None => None,
    };
    let Some(project_base) = project_base else {
        return Err(dirty(
            "checkout_not_the_starts",
            "the session has no project to remove its checkout from",
        ));
    };
    let Some((worktree_id, worktree_path)) =
        crate::service::safe_kill::removable_worktree(s, Some(wid), Some(&project_base))?
    else {
        return Err(dirty(
            "checkout_not_the_starts",
            "the session's checkout is the project's main checkout",
        ));
    };
    Ok(AbandonPlan {
        session_id,
        host_alias: row.host_alias,
        tmux_name: row.tmux_name,
        worktree_id,
        worktree_path,
        project_base,
        branch,
    })
}

/// PURE: the one round trip that inspects the checkout — its changes, then
/// the number of commits on HEAD that no other branch and no remote has.
fn inspect_script(p: &AbandonPlan) -> String {
    format!(
        "cd {wt} && git status --porcelain && echo {SEP} && \
         git rev-list --count HEAD --not --exclude={branch} --branches --remotes",
        wt = quote(&p.worktree_path),
        branch = quote(&p.branch),
    )
}

/// What [`inspect_script`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Inspection {
    changed: Vec<String>,
    own_commits: u64,
}

/// PURE: read [`inspect_script`]'s output; `None` when it is not that shape.
fn parse_inspection(stdout: &str) -> Option<Inspection> {
    let (status, count) = stdout.split_once(SEP)?;
    let changed = status
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim_end().to_string())
        .collect();
    let own_commits = count.trim().parse().ok()?;
    Some(Inspection {
        changed,
        own_commits,
    })
}

/// Check the checkout over `exec`: `Ok` only when it is clean and carries
/// no commit of its own.
async fn require_clean(exec: &dyn SshExec, p: &AbandonPlan) -> Result<(), IpcError> {
    let out = crate::ssh::run_shell(
        exec,
        &p.host_alias,
        &inspect_script(p),
        Duration::from_secs(30),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let found = match (out.status.success(), parse_inspection(&stdout)) {
        (true, Some(found)) => found,
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(dirty(
                "inspect_failed",
                format!(
                    "could not check the start's checkout, so it is kept: {}",
                    stderr.trim()
                ),
            ));
        }
    };
    if found.changed.is_empty() && found.own_commits == 0 {
        return Ok(());
    }
    let mut parts = Vec::new();
    if !found.changed.is_empty() {
        parts.push(format!("{} changed file(s)", found.changed.len()));
    }
    if found.own_commits > 0 {
        parts.push(format!("{} commit(s) of its own", found.own_commits));
    }
    Err(IpcError::new(
        codes::E_DIRTY,
        format!(
            "the start's checkout has {}; end the session from the sidebar instead",
            parts.join(" and ")
        ),
    )
    .with_details(serde_json::json!({
        "reason": "work_in_checkout",
        "changed": found.changed.iter().take(DIRTY_NAMED_MAX).collect::<Vec<_>>(),
        "changed_count": found.changed.len(),
        "own_commits": found.own_commits,
    })))
}

/// Cancel `session_id`'s start. The caller has already gated the session
/// (host fence, person tier `own`) and, for the operator, confirmed it.
pub async fn abandon_start(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    session_id: i64,
) -> Result<AbandonOutcome, IpcError> {
    let plan = plan_abandon(&*lock(store)?, session_id)?;
    require_clean(ssh.as_ref(), &plan).await?;
    if let Ok(s) = store.lock() {
        let _ = s.insert_session_event(session_id, START_ABANDONED, Some(&plan.branch));
    }
    // Kill first: nothing writes into the checkout after this.
    crate::service::sessions::kill_session(
        crate::service::sessions::KillSessionArgs {
            host_alias: plan.host_alias.clone(),
            name: plan.tmux_name.clone(),
            force: true,
        },
        store,
        ssh,
    )
    .await?;
    let remove = format!(
        "git -C {} worktree remove {}",
        quote(&plan.project_base),
        quote(&plan.worktree_path)
    );
    let out = crate::ssh::run_shell(
        ssh.as_ref(),
        &plan.host_alias,
        &remove,
        Duration::from_secs(30),
    )
    .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_WORKTREE_REMOVE,
            format!(
                "the session is ended, but its checkout was kept: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let fp_keys = Store::fingerprint_keys_of_worktree(store, plan.worktree_id);
    if let Err(e) = lock(store)?.delete_worktree(plan.worktree_id, &fp_keys) {
        tracing::warn!(
            worktree_id = plan.worktree_id,
            error = %e,
            "[abandon_start] dropping the worktree row failed"
        );
    }
    let branch = format!(
        "git -C {} branch -d -- {}",
        quote(&plan.project_base),
        quote(&plan.branch)
    );
    let branch_deleted = matches!(
        crate::ssh::run_shell(ssh.as_ref(), &plan.host_alias, &branch, Duration::from_secs(30)).await,
        Ok(out) if out.status.success()
    );
    Ok(AbandonOutcome {
        session_id,
        worktree_removed: true,
        branch_deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    fn plan() -> AbandonPlan {
        AbandonPlan {
            session_id: 1,
            host_alias: "h".into(),
            tmux_name: "pay-142".into(),
            worktree_id: 3,
            worktree_path: "/r/.worktrees/pay-142".into(),
            project_base: "/r".into(),
            branch: "pay-142".into(),
        }
    }

    #[test]
    fn the_inspection_counts_changes_and_the_heads_own_commits() {
        let s = inspect_script(&plan());
        assert!(s.starts_with("cd '/r/.worktrees/pay-142' && git status --porcelain"));
        assert!(
            s.contains("git rev-list --count HEAD --not --exclude='pay-142' --branches --remotes"),
            "{s}"
        );
        assert_eq!(
            parse_inspection(&format!("{SEP}\n0\n")),
            Some(Inspection {
                changed: vec![],
                own_commits: 0
            })
        );
        assert_eq!(
            parse_inspection(&format!(" M src/a.rs\n?? b.txt\n{SEP}\n2\n")),
            Some(Inspection {
                changed: vec![" M src/a.rs".into(), "?? b.txt".into()],
                own_commits: 2
            })
        );
        assert_eq!(parse_inspection("fatal: not a git repository"), None);
    }

    #[test]
    fn only_a_literal_branch_is_checked() {
        assert!(literal_branch("feat/imports-2"));
        assert!(!literal_branch(""));
        assert!(!literal_branch("feat/*"));
    }

    /// Work in the checkout refuses the cancel with what was found, and
    /// the only command run is the inspection: nothing is killed or removed.
    #[tokio::test]
    async fn a_checkout_with_work_in_it_is_never_removed() {
        let ssh = FakeSsh::new();
        ssh.on(
            Match::contains("git status --porcelain"),
            Reply::ok(&format!(" M src/a.rs\n{SEP}\n1\n")),
        );
        let err = require_clean(&ssh, &plan()).await.unwrap_err();
        assert_eq!(err.code, codes::E_DIRTY);
        let d = err.details.expect("details");
        assert_eq!(d["reason"], "work_in_checkout");
        assert_eq!(d["changed"], serde_json::json!([" M src/a.rs"]));
        assert_eq!(d["own_commits"], 1);
        assert_eq!(ssh.calls().len(), 1);

        let clean = FakeSsh::new();
        clean.on(
            Match::contains("git status --porcelain"),
            Reply::ok(&format!("{SEP}\n0\n")),
        );
        require_clean(&clean, &plan()).await.expect("clean");

        let broken = FakeSsh::new();
        broken.on(
            Match::contains("git status --porcelain"),
            Reply::fail(128, "fatal: not a git repository"),
        );
        let err = require_clean(&broken, &plan()).await.unwrap_err();
        assert_eq!(err.code, codes::E_DIRTY, "an unreadable checkout is kept");
    }
}
