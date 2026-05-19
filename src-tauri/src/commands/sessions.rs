use crate::ipc_error::IpcError;
use crate::store::{SessionRow, Store};
use crate::tmux::{
    kill_session as tmux_kill_session, list_local_sessions, new_session as tmux_new_session,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

const LOCAL_HOST: &str = "local";

fn reconcile_local_sessions(s: &Store) -> Result<Vec<SessionRow>, IpcError> {
    let live = list_local_sessions()?;
    s.upsert_host(LOCAL_HOST)?;
    let mut keep = Vec::with_capacity(live.len());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for sess in &live {
        keep.push(sess.name.clone());
        // Best-effort project mapping: find a project whose base_path is a prefix
        // of the session's working directory. Currently None until Phase 3 wires
        // attach-with-project; touch_project_last_session_at runs only when the
        // mapping is known.
        let project_id = find_project_id_for_path(s, &sess.path);
        s.upsert_session(
            &sess.name,
            LOCAL_HOST,
            project_id,
            None,
            sess.created,
            sess.last_activity,
            "running",
        )?;
        if let Some(pid) = project_id {
            s.touch_project_last_session_at(pid, now)?;
        }
    }
    s.delete_sessions_not_in(LOCAL_HOST, &keep)?;
    s.list_sessions_for_host(LOCAL_HOST).map_err(IpcError::from)
}

fn find_project_id_for_path(s: &Store, path: &std::path::Path) -> Option<i64> {
    let path_str = path.to_string_lossy();
    let projects = s.list_projects().ok()?;
    // Prefer the longest matching base_path (handles worktrees that live under repos)
    projects
        .into_iter()
        .filter(|p| path_str.starts_with(&p.base_path))
        .max_by_key(|p| p.base_path.len())
        .map(|p| p.id)
}

#[tauri::command]
pub fn list_sessions(store: State<'_, Mutex<Store>>) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_local_sessions(&s)
}

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
}

#[tauri::command]
pub fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<SessionRow, IpcError> {
    let path: PathBuf = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        if let Some(wid) = args.worktree_id {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT path FROM worktrees WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![wid], |r| r.get(0))?;
            PathBuf::from(row)
        } else {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT base_path FROM projects WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
            PathBuf::from(row)
        }
    };
    tmux_new_session(&args.name, &path)?;

    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_local_sessions(&s)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.name)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!("tmux session {} did not appear in list", args.name),
            )
        })
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub name: String,
}

#[tauri::command]
pub fn kill_session(args: KillSessionArgs, store: State<'_, Mutex<Store>>) -> Result<(), IpcError> {
    tmux_kill_session(&args.name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_local_sessions(&s)?;
    Ok(())
}
