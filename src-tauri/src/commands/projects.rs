use crate::ipc_error::IpcError;
use crate::projects::{list_worktrees, scan_projects};
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

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

#[tauri::command]
pub fn list_projects(store: State<'_, Mutex<Store>>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let projects = s.list_projects()?;
    let mut out = Vec::with_capacity(projects.len());
    for p in projects {
        let wts = s.list_worktrees_for_project(p.id)?;
        out.push(ProjectTreeRow {
            project: p,
            worktrees: wts,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn refresh_projects(store: State<'_, Mutex<Store>>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let base = projects_base();
    let discovered = scan_projects(&base)?;
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        for dp in &discovered {
            let project_id =
                s.upsert_project(&dp.owner, &dp.repo, &dp.base_path.to_string_lossy())?;
            let worktrees = match list_worktrees(&dp.base_path) {
                Ok(v) => v,
                Err(_) => continue, // Skip projects where `git worktree list` failed
            };
            let mut keep_names = Vec::with_capacity(worktrees.len());
            for wt in &worktrees {
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
    list_projects(store)
}
