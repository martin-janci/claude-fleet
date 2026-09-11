//! Tauri IPC wrappers for the Tasks panel (Wave 3 Track E / PROD-6). Logic
//! lives in `service::tasks`; these only adapt `tauri::State`. The desktop
//! is the master caller, so no per-host scoping applies here.

use crate::ipc_error::IpcError;
use crate::service::tasks;
use crate::store::{Store, TaskRow};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Tasks newest-first, optionally narrowed by requester and/or state.
#[tauri::command]
pub fn list_tasks(
    requester_session_id: Option<i64>,
    state: Option<String>,
    limit: Option<i64>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<TaskRow>, IpcError> {
    tasks::list_tasks_for(
        &store,
        requester_session_id,
        state.as_deref(),
        limit.unwrap_or(200).max(1),
        None,
    )
}

/// Cancel a queued / running task (`E_TASK_TERMINAL` when it already
/// finished). The worker session is left running.
#[tauri::command]
pub fn cancel_task(task_id: i64, store: State<'_, Arc<Mutex<Store>>>) -> Result<TaskRow, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    tasks::cancel_task(&s, task_id, "cancelled from the desktop")
}
