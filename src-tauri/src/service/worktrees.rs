//! Standalone worktree management: list worktrees with their "occupied by an
//! alive Claude session" status, and delete a worktree off the remote host +
//! drop the DB row. Refuses to delete an occupied worktree unless `force` is
//! passed.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{Store, WorktreeRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

fn lock_err() -> IpcError {
    IpcError::new("E_LOCK", "store mutex poisoned")
}

/// One worktree row with its alive-session occupants attached. `occupants`
/// is empty when the worktree is free to delete.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeOccupancy {
    pub worktree: WorktreeRow,
    pub occupants: Vec<WorktreeOccupant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeOccupant {
    pub host_alias: String,
    pub tmux_name: String,
}

#[derive(Deserialize)]
pub struct ListWorktreesArgs {
    /// Filter to one project; omit for all projects across the fleet.
    pub project_id: Option<i64>,
}

pub fn list_worktrees(
    args: ListWorktreesArgs,
    store: &Mutex<Store>,
) -> Result<Vec<WorktreeOccupancy>, IpcError> {
    let s = store.lock().map_err(|_| lock_err())?;
    let projects = s.list_projects().map_err(IpcError::from)?;
    let mut out = Vec::new();
    for proj in projects {
        if let Some(pid) = args.project_id {
            if proj.id != pid {
                continue;
            }
        }
        let worktrees = s
            .list_worktrees_for_project(proj.id)
            .map_err(IpcError::from)?;
        for wt in worktrees {
            let occupants = s
                .alive_sessions_for_worktree(wt.id)
                .map_err(IpcError::from)?
                .into_iter()
                .map(|(host_alias, tmux_name)| WorktreeOccupant {
                    host_alias,
                    tmux_name,
                })
                .collect();
            out.push(WorktreeOccupancy {
                worktree: wt,
                occupants,
            });
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct DeleteWorktreeArgs {
    pub worktree_id: i64,
    /// Bypass the alive-session occupant guard. The git-level dirty/conflict
    /// check still applies. Off by default.
    #[serde(default)]
    pub force: bool,
}

/// Delete a worktree on the remote host (via `git worktree remove`) and drop
/// the DB row. Refuses if any alive session is currently attached to the
/// worktree unless `force == true`.
///
/// Errors with `E_WORKTREE_BUSY` (occupied), `E_NOTFOUND` (no such row),
/// or `E_GIT` (git command failed — typically dirty tree or untracked files).
pub async fn delete_worktree(
    args: DeleteWorktreeArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    // Resolve everything we need under one lock; the SSH call below runs
    // off-lock.
    let (worktree_path, project_base_path, host_alias) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let wt = s
            .get_worktree_row(args.worktree_id)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("worktree {} not found", args.worktree_id),
                )
            })?;
        if !args.force {
            let occupants = s
                .alive_sessions_for_worktree(wt.id)
                .map_err(IpcError::from)?;
            if !occupants.is_empty() {
                let who = occupants
                    .iter()
                    .map(|(h, n)| format!("{h}/{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(IpcError::new(
                    "E_WORKTREE_BUSY",
                    format!("worktree is in use by session(s): {who}"),
                ));
            }
        }
        let proj_base = s
            .project_base_path(wt.project_id)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("project {} for worktree has no base path", wt.project_id),
                )
            })?;
        // Pick a host to run `git worktree remove` on. Prefer any session's
        // host (alive or not) so we hit the box where the worktree lives;
        // fall back to "local" when no session row remembers it.
        let host = s
            .alive_sessions_for_worktree(wt.id)
            .map_err(IpcError::from)?
            .first()
            .map(|(h, _)| h.clone())
            .unwrap_or_else(|| "local".to_string());
        (wt.path, proj_base, host)
    };

    // Run `git worktree remove`. No --force: respect uncommitted work. The
    // user can pass `force=true` for the occupant check, but the git-level
    // dirty guard is intentional — we never want to silently lose work.
    let cmd = format!(
        "git -C {} worktree remove {}",
        quote(&project_base_path),
        quote(&worktree_path)
    );
    let out = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-lc", &cmd])
            .output()
            .await
            .map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            &host_alias,
            &["bash", "-lc", &quote(&cmd)],
            std::time::Duration::from_secs(30),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            "E_GIT",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    let s = store.lock().map_err(|_| lock_err())?;
    s.delete_worktree(args.worktree_id)
        .map_err(IpcError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn list_worktrees_reports_no_occupants_for_empty_db() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let out = list_worktrees(ListWorktreesArgs { project_id: None }, &store).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn list_worktrees_flags_alive_session_occupant() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid;
        let wid;
        {
            let s = store.lock().unwrap();
            s.upsert_host("alpha").unwrap();
            pid = s.upsert_project("o", "r", "/p").unwrap();
            wid = s
                .upsert_worktree(pid, "feat", "/p/.worktrees/feat", None)
                .unwrap();
            let sid = s
                .upsert_session("sess", "alpha", Some(pid), Some(wid), 0, 0, "running", None)
                .unwrap();
            // sanity
            assert!(sid > 0);
        }
        let out = list_worktrees(ListWorktreesArgs { project_id: None }, &store).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].worktree.id, wid);
        assert_eq!(out[0].occupants.len(), 1);
        assert_eq!(out[0].occupants[0].tmux_name, "sess");
    }
}
