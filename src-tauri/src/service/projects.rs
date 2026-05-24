use crate::ipc_error::IpcError;
use crate::projects::{list_worktrees, scan_projects};
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize)]
pub struct ProjectTreeRow {
    pub project: ProjectRow,
    pub worktrees: Vec<WorktreeRow>,
}

pub(crate) fn projects_base() -> PathBuf {
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

    // 4. Apply all writes under a single brief lock.
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        for (dp, worktrees) in &upserts {
            let project_id =
                s.upsert_project(&dp.owner, &dp.repo, &dp.base_path.to_string_lossy())?;
            let mut keep_names = Vec::with_capacity(worktrees.len());
            for wt in worktrees {
                keep_names.push(wt.name.clone());
                s.upsert_worktree(
                    project_id,
                    &wt.name,
                    &wt.path.to_string_lossy(),
                    wt.branch.as_deref(),
                )?;
            }
            s.delete_worktrees_not_in(project_id, &keep_names)?;
        }
    }

    // 5. Return the fresh list under one final brief lock.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_projects_joined()
}
