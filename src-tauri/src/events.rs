//! Typed Tauri event bus for delta-update store sync.
//!
//! The `Store` calls into an `EventBus` whenever a row is mutated; the production
//! impl (`AppHandleEventBus`, defined in `lib.rs::setup` per Task 10) forwards
//! each event to the frontend via `tauri::AppHandle::emit`. Tests use
//! `NoopEventBus` (silent) or `RecordingEventBus` (captures every emit for
//! assertion).

use crate::store::{AccountRow, HostRow, ProjectRow, SessionRow, WorktreeRow};
use serde::Serialize;

/// A row mutation captured during a batched write (e.g. reconcile's
/// per-host write-burst). The SQL is applied inside a transaction; the
/// corresponding event is held as a `RowChange` and only flushed to the
/// `EventBus` AFTER the transaction commits. This guarantees no event fires
/// for a change that gets rolled back. See `EventBus::emit_change`.
#[derive(Clone)]
pub enum RowChange {
    SessionCreated(SessionRow),
    SessionUpdated(SessionRow),
    SessionKilled(i64),
    HostProbed(HostRow),
    ProjectUpdated(ProjectRow),
}

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

    /// Flush a single deferred `RowChange` through the matching typed method.
    /// Used by batched (transactional) writes to emit AFTER commit. The
    /// default impl dispatches to the existing methods, so no bus needs to
    /// override it.
    fn emit_change(&self, change: &RowChange) {
        match change {
            RowChange::SessionCreated(r) => self.session_created(r),
            RowChange::SessionUpdated(r) => self.session_updated(r),
            RowChange::SessionKilled(id) => self.session_killed(*id),
            RowChange::HostProbed(r) => self.host_probed(r),
            RowChange::ProjectUpdated(r) => self.project_updated(r),
        }
    }
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

/// Production event bus: forwards every event to the Tauri frontend.
///
/// Events are serialized immediately (a cheap in-memory operation) and handed
/// to a dedicated drain thread over an mpsc channel; the thread performs the
/// actual `AppHandle::emit`. This matters because the `Store` is mutated
/// while its `Mutex` is held: a bus call must NOT block, or it would stall
/// every other thread waiting on `store.lock()` for the duration of the
/// emit. A channel `send` is effectively instant.
///
/// Ordering is preserved (single channel, single consumer). Emit errors are
/// intentionally swallowed — if the webview isn't ready the Store mutation
/// has already committed and we don't want to roll it back.
pub struct AppHandleEventBus {
    // `mpsc::Sender` is `Send` but not `Sync`; the `EventBus` trait requires
    // `Sync`, so the sender lives behind a `Mutex`. The lock is held only for
    // the duration of a non-blocking `send`, so contention is negligible.
    tx: std::sync::Mutex<std::sync::mpsc::Sender<(&'static str, serde_json::Value)>>,
}

impl AppHandleEventBus {
    pub fn new(handle: tauri::AppHandle) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(&'static str, serde_json::Value)>();
        std::thread::spawn(move || {
            // Lives for the lifetime of the app; exits when the bus (and so
            // the Sender) is dropped and `recv` returns Err.
            while let Ok((name, payload)) = rx.recv() {
                let _ = tauri::Emitter::emit(&handle, name, payload);
            }
        });
        Self {
            tx: std::sync::Mutex::new(tx),
        }
    }

    fn queue<T: Serialize>(&self, name: &'static str, payload: &T) {
        // `to_value` on these small structs cannot realistically fail; on the
        // off chance it does we send Null rather than panic.
        let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send((name, value));
        }
    }
}

impl EventBus for AppHandleEventBus {
    fn session_created(&self, row: &SessionRow) {
        self.queue("session:created", row);
    }
    fn session_updated(&self, row: &SessionRow) {
        self.queue("session:updated", row);
    }
    fn session_killed(&self, id: i64) {
        self.queue("session:killed", &SessionKilledPayload { id });
    }
    fn host_added(&self, row: &HostRow) {
        self.queue("host:added", row);
    }
    fn host_probed(&self, row: &HostRow) {
        self.queue("host:probed", row);
    }
    fn host_removed(&self, alias: &str) {
        self.queue(
            "host:removed",
            &HostRemovedPayload { alias: alias.to_string() },
        );
    }
    fn account_upserted(&self, row: &AccountRow) {
        self.queue("account:upserted", row);
    }
    fn project_updated(&self, row: &ProjectRow) {
        self.queue("project:updated", row);
    }
    fn worktree_updated(&self, row: &WorktreeRow) {
        self.queue("worktree:updated", row);
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
