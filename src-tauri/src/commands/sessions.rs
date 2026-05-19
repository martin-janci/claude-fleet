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

#[tauri::command]
pub fn list_sessions(store: State<'_, Mutex<Store>>) -> Result<Vec<SessionRow>, IpcError> {
    let live = list_local_sessions()?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.upsert_host(LOCAL_HOST)?;
    let mut keep = Vec::with_capacity(live.len());
    for sess in &live {
        keep.push(sess.name.clone());
        s.upsert_session(
            &sess.name,
            LOCAL_HOST,
            None,
            None,
            sess.created,
            sess.last_activity,
            "running",
        )?;
    }
    s.delete_sessions_not_in(LOCAL_HOST, &keep)?;
    s.list_sessions_for_host(LOCAL_HOST).map_err(IpcError::from)
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
    // Reconcile the store with live tmux state and return the new row.
    let live = list_local_sessions()?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.upsert_host(LOCAL_HOST)?;
    let mut keep = Vec::with_capacity(live.len());
    for sess in &live {
        keep.push(sess.name.clone());
        s.upsert_session(
            &sess.name,
            LOCAL_HOST,
            None,
            None,
            sess.created,
            sess.last_activity,
            "running",
        )?;
    }
    s.delete_sessions_not_in(LOCAL_HOST, &keep)?;
    let rows = s.list_sessions_for_host(LOCAL_HOST)?;
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
    // Reconcile: re-list live sessions and prune the killed one.
    let live = list_local_sessions()?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.upsert_host(LOCAL_HOST)?;
    let mut keep = Vec::with_capacity(live.len());
    for sess in &live {
        keep.push(sess.name.clone());
        s.upsert_session(
            &sess.name,
            LOCAL_HOST,
            None,
            None,
            sess.created,
            sess.last_activity,
            "running",
        )?;
    }
    s.delete_sessions_not_in(LOCAL_HOST, &keep)?;
    Ok(())
}
