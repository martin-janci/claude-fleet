use crate::ipc_error::IpcError;
use crate::projects::{list_worktrees, scan_projects};
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Serialize)]
pub struct ProjectTreeRow {
    pub project: ProjectRow,
    pub worktrees: Vec<WorktreeRow>,
}

fn projects_base() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDE_FLEET_PROJECTS_BASE") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    PathBuf::from(home).join("projects").join("github.com")
}

pub fn list_projects(store: &Mutex<Store>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_projects_joined()
}

pub async fn refresh_projects(store: &Mutex<Store>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let base = projects_base();

    // 1. Snapshot the current project list under a brief lock.
    let snapshot = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_projects_joined()?
    };

    // 2. Scan the filesystem for new/removed projects (IO-only, no lock needed).
    let discovered = scan_projects(&base)?;

    // 3. Fan-out: run `git worktree list` for each discovered project, off-lock
    //    and in parallel using tokio tasks.
    let mut set = tokio::task::JoinSet::new();
    for dp in discovered.iter().cloned() {
        let _ = &snapshot; // borrow-check: snapshot not moved into tasks
        set.spawn(async move {
            let result = list_worktrees(&dp.base_path);
            (dp, result)
        });
    }

    // Collect git results off-lock.
    let mut upserts: Vec<(
        crate::projects::DiscoveredProject,
        Vec<crate::projects::DiscoveredWorktree>,
    )> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((dp, Ok(worktrees))) = joined {
            upserts.push((dp, worktrees));
        }
        // If the join errored or `list_worktrees` failed, skip — same behaviour
        // as the original `Err(_) => continue`.
    }

    // 4. Apply all writes under a single brief lock. Worktrees whose dir is gone
    //    (git `prunable`, or path missing on disk) are marked `missing_since`
    //    on first sight and auto-pruned on a later cycle — mirroring the
    //    ghost-session lifecycle. The main checkout is never treated as missing.
    let now = now_unix();
    let mut prune_repos: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        for (dp, worktrees) in &upserts {
            let project_id =
                s.upsert_project(&dp.owner, &dp.repo, &dp.base_path.to_string_lossy())?;
            let mut keep_names = Vec::with_capacity(worktrees.len());
            for wt in worktrees {
                let missing = wt.name != "main" && (wt.is_prunable || !wt.path.exists());
                // Read prior state BEFORE upsert (upsert clears missing_since).
                let prior_missing = s
                    .get_worktree_by_name(project_id, &wt.name)?
                    .and_then(|r| r.missing_since)
                    .is_some();
                if !missing {
                    s.upsert_worktree(
                        project_id,
                        &wt.name,
                        &wt.path.to_string_lossy(),
                        wt.branch.as_deref(),
                    )?;
                    keep_names.push(wt.name.clone());
                } else if !prior_missing {
                    // Phase 1: ensure the row exists, then stamp missing_since.
                    s.upsert_worktree(
                        project_id,
                        &wt.name,
                        &wt.path.to_string_lossy(),
                        wt.branch.as_deref(),
                    )?;
                    s.mark_worktree_missing(project_id, &wt.name, now)?;
                    keep_names.push(wt.name.clone());
                } else {
                    // Phase 2: still missing — prune row + ghost sessions; clear
                    // git's registration off-lock below. Not added to keep_names.
                    if let Some(row) = s.get_worktree_by_name(project_id, &wt.name)? {
                        s.prune_missing_worktree(row.id, now)?;
                        prune_repos.insert(dp.base_path.clone());
                    }
                }
            }
            s.delete_worktrees_not_in(project_id, &keep_names)?;
        }
    }

    // 4b. Clear git's stale worktree registrations off-lock (so subsequent
    //     `git -C <gone-worktree>` calls stop erroring). Best-effort.
    for repo in &prune_repos {
        let _ = tokio::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "prune"])
            .output()
            .await;
    }

    // 5. Return the fresh list under one final brief lock.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_projects_joined()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[tokio::test]
    async fn missing_worktree_is_marked_then_pruned() {
        let base = TempDir::new().unwrap();
        let repo = base.path().join("owner").join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "init",
            ],
        );
        let wt = repo.join(".worktrees").join("feat");
        git(
            &repo,
            &["worktree", "add", wt.to_str().unwrap(), "-b", "feat"],
        );
        std::fs::remove_dir_all(&wt).unwrap();

        std::env::set_var("CLAUDE_FLEET_PROJECTS_BASE", base.path());
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());

        // Refresh #1: worktree dir gone → marked missing (row kept).
        refresh_projects(&store).await.unwrap();
        let tree = list_projects(&store).unwrap();
        let feat = tree[0]
            .worktrees
            .iter()
            .find(|w| w.name == "feat")
            .expect("feat present after refresh 1");
        assert!(feat.missing_since.is_some());

        // Refresh #2: still missing → pruned (row gone).
        refresh_projects(&store).await.unwrap();
        let tree = list_projects(&store).unwrap();
        assert!(tree[0].worktrees.iter().all(|w| w.name != "feat"));

        std::env::remove_var("CLAUDE_FLEET_PROJECTS_BASE");
    }
}
