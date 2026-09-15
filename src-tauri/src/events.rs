//! Typed Tauri event bus for delta-update store sync.
//!
//! The `Store` calls into an `EventBus` whenever a row is mutated; the production
//! impl (`AppHandleEventBus`, defined in `lib.rs::setup` per Task 10) forwards
//! each event to the frontend via `tauri::AppHandle::emit`. Tests use
//! `NoopEventBus` (silent) or `RecordingEventBus` (captures every emit for
//! assertion).
//!
//! Every event is one [`RowChange`] variant. The typed convenience methods on
//! [`EventBus`] (`session_updated(&row)`, …) are default trait methods that
//! build the variant and hand it to the single required [`EventBus::emit`];
//! [`RowChange::name`] is the frontend event name `src/lib/events.ts`
//! subscribes to and [`RowChange::payload`] the JSON the frontend receives.

use crate::service::account_usage::AccountUsageSnapshot;
use crate::store::{
    AccountRow, AssetInventoryRow, HostRow, ProjectRow, SessionRow, TaskRow, WorktreeRow,
};
use serde::Serialize;

/// One event on the bus. Also used as the deferred form during a batched
/// write (e.g. reconcile's per-host write-burst): the SQL is applied inside a
/// transaction, the `RowChange`s are held, and only flushed to the `EventBus`
/// AFTER the transaction commits. This guarantees no event fires for a change
/// that gets rolled back. See `EventBus::emit_change`.
#[derive(Clone)]
pub enum RowChange {
    SessionCreated(SessionRow),
    SessionUpdated(SessionRow),
    SessionKilled(i64),
    HostAdded(HostRow),
    HostProbed(HostRow),
    HostRemoved(String),
    AccountUpserted(AccountRow),
    ProjectUpdated(ProjectRow),
    WorktreeUpdated(WorktreeRow),
    WorktreeRemoved(i64),
    /// A task row was created or changed state (migration 020). There is no
    /// `task:removed` — tasks only ever move to a terminal state.
    TaskUpdated(TaskRow),
    /// An account's usage snapshot changed (Task 4): a fetch that was due
    /// completed with a result different from what the cache already held.
    /// A call the floor turns away (not due, snapshot unchanged) never
    /// reaches this.
    AccountUsageUpdated(AccountUsageSnapshot),
    /// One asset's drift state on one host changed (migration 030).
    AssetInventoryUpdated(AssetInventoryRow),
    /// Every inventory row for (host, harness) was dropped before a rescan
    /// writes the new set.
    AssetInventoryCleared {
        host_alias: String,
        harness: String,
    },
    /// The catalog repo was (re)loaded; the summary carries its HEAD and
    /// counts. Not a store row.
    CatalogLoaded(CatalogSummary),
    /// Progress of an in-flight sync apply (sub-project 2). Not a store row.
    SyncProgress(SyncProgress),
}

#[derive(Serialize, Clone)]
pub struct SessionKilledPayload {
    pub id: i64,
}

#[derive(Serialize, Clone)]
pub struct HostRemovedPayload {
    pub alias: String,
}

#[derive(Serialize, Clone)]
pub struct WorktreeRemovedPayload {
    pub id: i64,
}

#[derive(Serialize, Clone)]
pub struct AssetInventoryClearedPayload {
    pub host_alias: String,
    pub harness: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CatalogSummary {
    pub head: String,
    pub loaded_at: i64,
    pub asset_count: usize,
    pub problem_count: usize,
}

/// Progress of one in-flight sync apply (migration 031 / sub-project 2):
/// (host, harness) pairs finished / total pairs in the plan;
/// `host_alias`/`harness` name the pair about to be applied. A run that
/// completes ends with one terminal event at `done == total` whose
/// `host_alias`/`harness` are empty — no pair is about to be applied.
#[derive(Serialize, Clone, Debug)]
pub struct SyncProgress {
    pub plan_id: String,
    pub host_alias: String,
    pub harness: String,
    pub done: usize,
    pub total: usize,
}

impl RowChange {
    /// The frontend event name (`src/lib/events.ts` `RowEvent.name`).
    pub fn name(&self) -> &'static str {
        match self {
            RowChange::SessionCreated(_) => "session:created",
            RowChange::SessionUpdated(_) => "session:updated",
            RowChange::SessionKilled(_) => "session:killed",
            RowChange::HostAdded(_) => "host:added",
            RowChange::HostProbed(_) => "host:probed",
            RowChange::HostRemoved(_) => "host:removed",
            RowChange::AccountUpserted(_) => "account:upserted",
            RowChange::ProjectUpdated(_) => "project:updated",
            RowChange::WorktreeUpdated(_) => "worktree:updated",
            RowChange::WorktreeRemoved(_) => "worktree:removed",
            RowChange::TaskUpdated(_) => "task:updated",
            RowChange::AccountUsageUpdated(_) => "account_usage:updated",
            RowChange::AssetInventoryUpdated(_) => "asset_inventory:updated",
            RowChange::AssetInventoryCleared { .. } => "asset_inventory:cleared",
            RowChange::CatalogLoaded(_) => "catalog:loaded",
            RowChange::SyncProgress(_) => "sync:progress",
        }
    }

    /// The JSON payload the frontend receives for this event.
    pub fn payload(&self) -> serde_json::Value {
        fn to_value<T: Serialize>(v: &T) -> serde_json::Value {
            // `to_value` on these small structs cannot realistically fail;
            // on the off chance it does we send Null rather than panic.
            serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
        }
        match self {
            RowChange::SessionCreated(r) | RowChange::SessionUpdated(r) => to_value(r),
            RowChange::SessionKilled(id) => to_value(&SessionKilledPayload { id: *id }),
            RowChange::HostAdded(r) | RowChange::HostProbed(r) => to_value(r),
            RowChange::HostRemoved(alias) => to_value(&HostRemovedPayload {
                alias: alias.clone(),
            }),
            RowChange::AccountUpserted(r) => to_value(r),
            RowChange::ProjectUpdated(r) => to_value(r),
            RowChange::WorktreeUpdated(r) => to_value(r),
            RowChange::WorktreeRemoved(id) => to_value(&WorktreeRemovedPayload { id: *id }),
            RowChange::TaskUpdated(r) => to_value(r),
            RowChange::AccountUsageUpdated(r) => to_value(r),
            RowChange::AssetInventoryUpdated(r) => to_value(r),
            RowChange::AssetInventoryCleared {
                host_alias,
                harness,
            } => to_value(&AssetInventoryClearedPayload {
                host_alias: host_alias.clone(),
                harness: harness.clone(),
            }),
            RowChange::CatalogLoaded(s) => to_value(s),
            RowChange::SyncProgress(p) => to_value(p),
        }
    }
}

pub trait EventBus: Send + Sync {
    /// The one method an impl provides: deliver a single event.
    fn emit(&self, e: &RowChange);

    fn session_created(&self, row: &SessionRow) {
        self.emit(&RowChange::SessionCreated(row.clone()));
    }
    fn session_updated(&self, row: &SessionRow) {
        self.emit(&RowChange::SessionUpdated(row.clone()));
    }
    fn session_killed(&self, id: i64) {
        self.emit(&RowChange::SessionKilled(id));
    }
    fn host_added(&self, row: &HostRow) {
        self.emit(&RowChange::HostAdded(row.clone()));
    }
    fn host_probed(&self, row: &HostRow) {
        self.emit(&RowChange::HostProbed(row.clone()));
    }
    fn host_removed(&self, alias: &str) {
        self.emit(&RowChange::HostRemoved(alias.to_string()));
    }
    fn account_upserted(&self, row: &AccountRow) {
        self.emit(&RowChange::AccountUpserted(row.clone()));
    }
    fn project_updated(&self, row: &ProjectRow) {
        self.emit(&RowChange::ProjectUpdated(row.clone()));
    }
    fn worktree_updated(&self, row: &WorktreeRow) {
        self.emit(&RowChange::WorktreeUpdated(row.clone()));
    }
    fn worktree_removed(&self, id: i64) {
        self.emit(&RowChange::WorktreeRemoved(id));
    }
    /// See [`RowChange::TaskUpdated`].
    fn task_updated(&self, row: &TaskRow) {
        self.emit(&RowChange::TaskUpdated(row.clone()));
    }
    /// See [`RowChange::AccountUsageUpdated`].
    fn account_usage_updated(&self, row: &AccountUsageSnapshot) {
        self.emit(&RowChange::AccountUsageUpdated(row.clone()));
    }
    /// See [`RowChange::AssetInventoryUpdated`].
    fn asset_inventory_updated(&self, row: &AssetInventoryRow) {
        self.emit(&RowChange::AssetInventoryUpdated(row.clone()));
    }
    /// See [`RowChange::AssetInventoryCleared`].
    fn asset_inventory_cleared(&self, host_alias: &str, harness: &str) {
        self.emit(&RowChange::AssetInventoryCleared {
            host_alias: host_alias.to_string(),
            harness: harness.to_string(),
        });
    }
    /// See [`RowChange::CatalogLoaded`].
    fn catalog_loaded(&self, summary: &CatalogSummary) {
        self.emit(&RowChange::CatalogLoaded(summary.clone()));
    }
    /// See [`RowChange::SyncProgress`].
    fn sync_progress(&self, p: &SyncProgress) {
        self.emit(&RowChange::SyncProgress(p.clone()));
    }

    /// Flush a single deferred `RowChange`. Used by batched (transactional)
    /// writes to emit AFTER commit; an alias of [`EventBus::emit`] kept for
    /// those call sites.
    fn emit_change(&self, change: &RowChange) {
        self.emit(change);
    }
}

/// Silently drops every event. For tests and any context that doesn't need
/// to surface row changes to a frontend.
pub struct NoopEventBus;
impl EventBus for NoopEventBus {
    fn emit(&self, _: &RowChange) {}
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
}

impl EventBus for AppHandleEventBus {
    fn emit(&self, e: &RowChange) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send((e.name(), e.payload()));
        }
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
    /// Records `<event name>:<row key>` — the same shape the store tests have
    /// always asserted on.
    fn emit(&self, e: &RowChange) {
        let key = match e {
            RowChange::SessionCreated(r) | RowChange::SessionUpdated(r) => r.id.to_string(),
            RowChange::SessionKilled(id) | RowChange::WorktreeRemoved(id) => id.to_string(),
            RowChange::HostAdded(r) | RowChange::HostProbed(r) => r.alias.clone(),
            RowChange::HostRemoved(alias) => alias.clone(),
            RowChange::AccountUpserted(r) => r.uuid.clone(),
            RowChange::ProjectUpdated(r) => r.id.to_string(),
            RowChange::WorktreeUpdated(r) => r.id.to_string(),
            RowChange::TaskUpdated(r) => format!("{}:{}", r.id, r.state),
            RowChange::AccountUsageUpdated(r) => r.account_uuid.clone(),
            RowChange::AssetInventoryUpdated(r) => {
                format!("{}:{}:{}:{}", r.host_alias, r.harness, r.kind, r.name)
            }
            RowChange::AssetInventoryCleared {
                host_alias,
                harness,
            } => format!("{host_alias}:{harness}"),
            RowChange::CatalogLoaded(s) => s.head.clone(),
            RowChange::SyncProgress(p) => {
                format!("{}:{}:{}/{}", p.host_alias, p.harness, p.done, p.total)
            }
        };
        self.events
            .lock()
            .unwrap()
            .push(format!("{}:{key}", e.name()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend event names, in `src/lib/events.ts` `RowEvent` order.
    /// These strings are what the frontend `listen`s to, so a rename on one
    /// side without the other silently drops the event.
    const FRONTEND_EVENT_NAMES: [&str; 16] = [
        "session:created",
        "session:updated",
        "session:killed",
        "host:added",
        "host:probed",
        "host:removed",
        "account:upserted",
        "project:updated",
        "worktree:updated",
        "worktree:removed",
        "task:updated",
        "account_usage:updated",
        "asset_inventory:updated",
        "asset_inventory:cleared",
        "catalog:loaded",
        "sync:progress",
    ];

    #[test]
    fn frontend_declares_every_event_name() {
        let events_ts = include_str!("../../src/lib/events.ts");
        for name in FRONTEND_EVENT_NAMES {
            assert!(
                events_ts.contains(&format!("'{name}'")),
                "src/lib/events.ts does not declare {name}"
            );
        }
    }

    /// `RowChange::name` for every variant a test can build without a full
    /// store row. The row-bearing variants (`SessionCreated`, `HostAdded`,
    /// `ProjectUpdated`, …) are pinned by the store tests, which assert the
    /// `RecordingEventBus` strings (`session:updated:<id>`, …) end to end.
    #[test]
    fn row_change_names_match_frontend_subscriptions() {
        let summary = CatalogSummary {
            head: String::new(),
            loaded_at: 0,
            asset_count: 0,
            problem_count: 0,
        };
        let progress = SyncProgress {
            plan_id: String::new(),
            host_alias: String::new(),
            harness: String::new(),
            done: 0,
            total: 0,
        };
        let cases: Vec<(RowChange, &str)> = vec![
            (RowChange::SessionKilled(1), "session:killed"),
            (RowChange::HostRemoved("h".into()), "host:removed"),
            (
                RowChange::AccountUpserted(AccountRow::default()),
                "account:upserted",
            ),
            (RowChange::WorktreeRemoved(1), "worktree:removed"),
            (
                RowChange::AssetInventoryUpdated(AssetInventoryRow::default()),
                "asset_inventory:updated",
            ),
            (
                RowChange::AssetInventoryCleared {
                    host_alias: "h".into(),
                    harness: "claude".into(),
                },
                "asset_inventory:cleared",
            ),
            (RowChange::CatalogLoaded(summary), "catalog:loaded"),
            (RowChange::SyncProgress(progress), "sync:progress"),
        ];
        for (change, expected) in &cases {
            assert_eq!(change.name(), *expected);
            assert!(FRONTEND_EVENT_NAMES.contains(expected));
        }
    }

    #[test]
    fn scalar_payloads_keep_their_wire_shape() {
        assert_eq!(
            RowChange::SessionKilled(7).payload(),
            serde_json::json!({ "id": 7 })
        );
        assert_eq!(
            RowChange::WorktreeRemoved(3).payload(),
            serde_json::json!({ "id": 3 })
        );
        assert_eq!(
            RowChange::HostRemoved("box".into()).payload(),
            serde_json::json!({ "alias": "box" })
        );
        assert_eq!(
            RowChange::AssetInventoryCleared {
                host_alias: "box".into(),
                harness: "claude".into(),
            }
            .payload(),
            serde_json::json!({ "host_alias": "box", "harness": "claude" })
        );
    }

    #[test]
    fn typed_methods_route_through_emit() {
        let bus = RecordingEventBus::new();
        bus.session_killed(5);
        bus.host_removed("box");
        bus.worktree_removed(9);
        bus.asset_inventory_cleared("box", "claude");
        assert_eq!(
            bus.take(),
            vec![
                "session:killed:5",
                "host:removed:box",
                "worktree:removed:9",
                "asset_inventory:cleared:box:claude",
            ]
        );
    }
}
