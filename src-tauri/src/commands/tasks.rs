//! Tauri IPC wrappers for the Tasks panel (Wave 3 Track E / PROD-6). Logic
//! lives in `service::tasks`; these only adapt `tauri::State`. Standalone the
//! desktop is the master caller, so no per-host scoping applies; pointed at a
//! hub both commands route to the matching tool.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::tasks;
use fleet_core::store::{Store, TaskRow};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Tasks newest-first, optionally narrowed by requester and/or state.
#[tauri::command]
pub async fn list_tasks(
    requester_session_id: Option<i64>,
    state: Option<String>,
    limit: Option<i64>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<TaskRow>, IpcError> {
    routed::list_tasks(&backend, requester_session_id, state, limit, &store).await
}

/// Cancel a queued / running task (`E_TASK_TERMINAL` when it already
/// finished). The worker session is left running.
#[tauri::command]
pub async fn cancel_task(
    task_id: i64,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TaskRow, IpcError> {
    routed::cancel_task(&backend, task_id, &store).await
}

pub(crate) mod routed {
    use super::*;

    /// The `limit` clamp is applied on this side for both backends, so the
    /// hub sees the same number the local store would have.
    pub async fn list_tasks(
        backend: &FleetBackend,
        requester_session_id: Option<i64>,
        state: Option<String>,
        limit: Option<i64>,
        store: &Mutex<Store>,
    ) -> Result<Vec<TaskRow>, IpcError> {
        let limit = limit.unwrap_or(200).max(1);
        match backend.hub() {
            Some(hub) => {
                hub.list_tasks(requester_session_id, state, Some(limit))
                    .await
            }
            None => {
                tasks::list_tasks_for(store, requester_session_id, state.as_deref(), limit, None)
            }
        }
    }

    pub async fn cancel_task(
        backend: &FleetBackend,
        task_id: i64,
        store: &Mutex<Store>,
    ) -> Result<TaskRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.cancel_task(task_id).await,
            None => {
                let s = lock(store)?;
                tasks::cancel_task(&s, task_id, "cancelled from the desktop")
            }
        }
    }
}
