//! Typed Tauri event bus for delta-update store sync.
//!
//! The `Store` calls into an `EventBus` whenever a row is mutated; the production
//! impl (`AppHandleEventBus`, defined in `lib.rs::setup` per Task 10) forwards
//! each event to the frontend via `tauri::AppHandle::emit`. Tests use
//! `NoopEventBus` (silent) or `RecordingEventBus` (captures every emit for
//! assertion).

use crate::store::{AccountRow, HostRow, ProjectRow, SessionRow, WorktreeRow};
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct SessionKilledPayload {
    pub id: i64,
}

#[derive(Serialize, Clone)]
pub struct HostRemovedPayload {
    pub alias: String,
}

pub trait EventBus: Send + Sync {
    fn session_created(&self, row: &SessionRow);
    fn session_updated(&self, row: &SessionRow);
    fn session_killed(&self, id: i64);
    fn host_added(&self, row: &HostRow);
    fn host_probed(&self, row: &HostRow);
    fn host_removed(&self, alias: &str);
    fn account_upserted(&self, row: &AccountRow);
    fn project_updated(&self, row: &ProjectRow);
    fn worktree_updated(&self, row: &WorktreeRow);
}

/// Silently drops every event. For tests and any context that doesn't need
/// to surface row changes to a frontend.
pub struct NoopEventBus;
impl EventBus for NoopEventBus {
    fn session_created(&self, _: &SessionRow) {}
    fn session_updated(&self, _: &SessionRow) {}
    fn session_killed(&self, _: i64) {}
    fn host_added(&self, _: &HostRow) {}
    fn host_probed(&self, _: &HostRow) {}
    fn host_removed(&self, _: &str) {}
    fn account_upserted(&self, _: &AccountRow) {}
    fn project_updated(&self, _: &ProjectRow) {}
    fn worktree_updated(&self, _: &WorktreeRow) {}
}

/// Production event bus: forwards every event to the Tauri frontend via
/// `AppHandle::emit`. Errors are intentionally swallowed — if the webview
/// isn't ready the Store mutation has already committed and we don't want
/// to roll it back.
pub struct AppHandleEventBus {
    handle: tauri::AppHandle,
}

impl AppHandleEventBus {
    pub fn new(handle: tauri::AppHandle) -> Self {
        Self { handle }
    }
}

impl EventBus for AppHandleEventBus {
    fn session_created(&self, row: &SessionRow) {
        let _ = tauri::Emitter::emit(&self.handle, "session:created", row);
    }
    fn session_updated(&self, row: &SessionRow) {
        let _ = tauri::Emitter::emit(&self.handle, "session:updated", row);
    }
    fn session_killed(&self, id: i64) {
        let _ = tauri::Emitter::emit(&self.handle, "session:killed", SessionKilledPayload { id });
    }
    fn host_added(&self, row: &HostRow) {
        let _ = tauri::Emitter::emit(&self.handle, "host:added", row);
    }
    fn host_probed(&self, row: &HostRow) {
        let _ = tauri::Emitter::emit(&self.handle, "host:probed", row);
    }
    fn host_removed(&self, alias: &str) {
        let _ = tauri::Emitter::emit(
            &self.handle,
            "host:removed",
            HostRemovedPayload { alias: alias.to_string() },
        );
    }
    fn account_upserted(&self, row: &AccountRow) {
        let _ = tauri::Emitter::emit(&self.handle, "account:upserted", row);
    }
    fn project_updated(&self, row: &ProjectRow) {
        let _ = tauri::Emitter::emit(&self.handle, "project:updated", row);
    }
    fn worktree_updated(&self, row: &WorktreeRow) {
        let _ = tauri::Emitter::emit(&self.handle, "worktree:updated", row);
    }
}

/// Records every event in order. Used in unit tests to assert that a Store
/// mutation produced the expected events.
#[cfg(test)]
pub struct RecordingEventBus {
    pub events: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl RecordingEventBus {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

#[cfg(test)]
impl EventBus for RecordingEventBus {
    fn session_created(&self, r: &SessionRow) {
        self.events.lock().unwrap().push(format!("session:created:{}", r.id));
    }
    fn session_updated(&self, r: &SessionRow) {
        self.events.lock().unwrap().push(format!("session:updated:{}", r.id));
    }
    fn session_killed(&self, id: i64) {
        self.events.lock().unwrap().push(format!("session:killed:{}", id));
    }
    fn host_added(&self, r: &HostRow) {
        self.events.lock().unwrap().push(format!("host:added:{}", r.alias));
    }
    fn host_probed(&self, r: &HostRow) {
        self.events.lock().unwrap().push(format!("host:probed:{}", r.alias));
    }
    fn host_removed(&self, alias: &str) {
        self.events.lock().unwrap().push(format!("host:removed:{}", alias));
    }
    fn account_upserted(&self, r: &AccountRow) {
        self.events.lock().unwrap().push(format!("account:upserted:{}", r.uuid));
    }
    fn project_updated(&self, r: &ProjectRow) {
        self.events.lock().unwrap().push(format!("project:updated:{}", r.id));
    }
    fn worktree_updated(&self, r: &WorktreeRow) {
        self.events.lock().unwrap().push(format!("worktree:updated:{}", r.id));
    }
}
