//! Typed event bus for delta-update store sync.
//!
//! The `Store` calls into an `EventBus` whenever a row is mutated; the desktop's
//! `AppHandleEventBus` lives in `src-tauri/src/app_events.rs` and forwards each
//! event to the Svelte frontend. Tests use `NoopEventBus` (silent) or
//! `RecordingEventBus` (captures every emit for assertion).
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

/// One event as it travels to a remote subscriber: the name and payload a
/// [`RowChange`] renders to, with the store row already serialized so nothing
/// downstream needs the row types. Cheap to clone — `Value` is the only
/// owned field, and a broadcast channel clones once per receiver.
#[derive(Clone, Debug)]
pub struct EventMessage {
    pub name: &'static str,
    pub payload: serde_json::Value,
}

impl EventMessage {
    /// The part of the name before `:` — `session`, `host`, `account_usage`,
    /// … — which is what the `/events` route's `?kinds=` filter matches on.
    pub fn kind(&self) -> &str {
        self.name
            .split_once(':')
            .map(|(k, _)| k)
            .unwrap_or(self.name)
    }
}

/// Fans every event out to any number of live subscribers over a
/// `tokio::sync::broadcast` channel. `fleet-hub serve` opens its store with
/// this bus and the `GET /events` SSE route subscribes per connection.
///
/// Like the desktop's `AppHandleEventBus` (which queues through an mpsc
/// channel and a drain thread), [`emit`](BroadcastEventBus::emit) never
/// blocks: `broadcast::Sender::send` writes into the ring buffer and returns
/// immediately, whether there are no subscribers at all or a slow one that
/// has fallen behind. A store write therefore never waits on delivery. The
/// price of that promise is the ring: a subscriber that falls more than
/// `capacity` events behind loses the oldest ones and is told so
/// (`RecvError::Lagged`) rather than holding anyone up.
pub struct BroadcastEventBus {
    tx: tokio::sync::broadcast::Sender<EventMessage>,
}

/// Every event kind — the part of a [`RowChange::name`] before the `:`, which
/// is what the `/events` route's `?kinds=` filter matches on.
/// `event_kinds_cover_every_name` keeps it in step with the variants.
pub const EVENT_KINDS: [&str; 10] = [
    "session",
    "host",
    "account",
    "project",
    "worktree",
    "task",
    "account_usage",
    "asset_inventory",
    "catalog",
    "sync",
];

/// Ring size for [`BroadcastEventBus`]. Reconcile holds its emits until the
/// transaction commits and then flushes them in one burst (one pass every
/// ~20 s on a hub), so the buffer has to absorb a whole pass over a busy
/// fleet, not a steady trickle.
pub const BROADCAST_CAPACITY: usize = 256;

impl BroadcastEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        Self { tx }
    }

    /// A receiver that sees every event emitted *after* this call. Nothing is
    /// replayed: a client that wants the current state lists it once and then
    /// follows the stream.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventMessage> {
        self.tx.subscribe()
    }

    /// Live subscribers. [`BroadcastEventBus::emit`] reads it to skip the
    /// work of rendering an event nobody is listening for.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for BroadcastEventBus {
    fn default() -> Self {
        Self::new(BROADCAST_CAPACITY)
    }
}

impl EventBus for BroadcastEventBus {
    fn emit(&self, e: &RowChange) {
        // The usual state of a hub nobody has connected a phone to. Checked
        // FIRST because `payload()` serializes a whole store row, and a
        // reconcile pass emits one per session on a busy fleet: without this,
        // every hub would pay to render a feed no one is reading. A
        // subscriber that arrives between this check and the `send` below
        // simply misses this one event, which is the same race `send` already
        // has and is exactly what "nothing is replayed" means here.
        if self.receiver_count() == 0 {
            return;
        }
        // `Err` means the last receiver went away in that window. Not an
        // error, not a log line.
        let _ = self.tx.send(EventMessage {
            name: e.name(),
            payload: e.payload(),
        });
    }
}

/// Records every event in order. Used in unit tests to assert that a Store
/// mutation produced the expected events.
/// Crate-private: `events` is a `pub` module now, and exporting a test-only
/// helper would put it in `fleet-core`'s public API.
#[cfg(test)]
pub(crate) struct RecordingEventBus {
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
        let events_ts = include_str!("../../../src/lib/events.ts");
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

    /// True when `EVENT_KINDS` lists the part of `name` before the `:`.
    /// A `const fn` so it can be evaluated at COMPILE time; const eval has no
    /// formatting, so the caller's `const` item is what names the offender in
    /// the compiler's message.
    const fn kind_is_listed(name: &str) -> bool {
        let nb = name.as_bytes();
        let mut n = 0;
        while n < nb.len() && nb[n] != b':' {
            n += 1;
        }
        if n == nb.len() {
            return false; // no `:` at all — not an event name
        }
        let mut k = 0;
        while k < EVENT_KINDS.len() {
            let kb = EVENT_KINDS[k].as_bytes();
            if kb.len() == n {
                let mut i = 0;
                while i < n && kb[i] == nb[i] {
                    i += 1;
                }
                if i == n {
                    return true;
                }
            }
            k += 1;
        }
        false
    }

    /// True when `FRONTEND_EVENT_NAMES` contains `name` exactly.
    const fn name_is_declared(name: &str) -> bool {
        let nb = name.as_bytes();
        let mut i = 0;
        while i < FRONTEND_EVENT_NAMES.len() {
            let fb = FRONTEND_EVENT_NAMES[i].as_bytes();
            if fb.len() == nb.len() {
                let mut j = 0;
                while j < nb.len() && fb[j] == nb[j] {
                    j += 1;
                }
                if j == nb.len() {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    /// An event name checked against both lists while the crate compiles.
    /// A name whose kind is not in [`EVENT_KINDS`] (it would be
    /// unsubscribable on `/events`) or which `FRONTEND_EVENT_NAMES` does not
    /// declare fails const evaluation — a compile error, not a test failure.
    macro_rules! pinned_name {
        ($name:literal) => {{
            const N: &str = {
                assert!(
                    kind_is_listed($name),
                    "EVENT_KINDS has no kind for this event name"
                );
                assert!(
                    name_is_declared($name),
                    "FRONTEND_EVENT_NAMES does not declare this event name"
                );
                $name
            };
            N
        }};
    }

    /// The compile-time pin for `/events` subscribability.
    ///
    /// `EVENT_KINDS` used to be cross-checked only against the hardcoded
    /// `FRONTEND_EVENT_NAMES` fixture, never against `RowChange` itself: a new
    /// variant added without touching that fixture compiled, passed the suite,
    /// and was silently unsubscribable (`?kinds=<its kind>` would be logged as
    /// unrecognised and drop every one of its events).
    ///
    /// The `match` below is exhaustive, so **a new variant does not compile**
    /// until it has an arm here; the arm's name is checked against both lists
    /// at compile time by `pinned_name!`. Adding a variant therefore forces
    /// `EVENT_KINDS`, `FRONTEND_EVENT_NAMES` and `RowChange::name` into step
    /// before anything builds.
    #[test]
    fn every_row_change_variant_is_subscribable() {
        fn pinned(c: &RowChange) -> &'static str {
            match c {
                RowChange::SessionCreated(_) => pinned_name!("session:created"),
                RowChange::SessionUpdated(_) => pinned_name!("session:updated"),
                RowChange::SessionKilled(_) => pinned_name!("session:killed"),
                RowChange::HostAdded(_) => pinned_name!("host:added"),
                RowChange::HostProbed(_) => pinned_name!("host:probed"),
                RowChange::HostRemoved(_) => pinned_name!("host:removed"),
                RowChange::AccountUpserted(_) => pinned_name!("account:upserted"),
                RowChange::ProjectUpdated(_) => pinned_name!("project:updated"),
                RowChange::WorktreeUpdated(_) => pinned_name!("worktree:updated"),
                RowChange::WorktreeRemoved(_) => pinned_name!("worktree:removed"),
                RowChange::TaskUpdated(_) => pinned_name!("task:updated"),
                RowChange::AccountUsageUpdated(_) => pinned_name!("account_usage:updated"),
                RowChange::AssetInventoryUpdated(_) => pinned_name!("asset_inventory:updated"),
                RowChange::AssetInventoryCleared { .. } => pinned_name!("asset_inventory:cleared"),
                RowChange::CatalogLoaded(_) => pinned_name!("catalog:loaded"),
                RowChange::SyncProgress(_) => pinned_name!("sync:progress"),
            }
        }
        // And for every variant a test can build without a full store row,
        // `RowChange::name` really is the name pinned above.
        for c in [
            RowChange::SessionKilled(1),
            RowChange::HostRemoved("h".into()),
            RowChange::AccountUpserted(AccountRow::default()),
            RowChange::WorktreeRemoved(1),
            RowChange::AssetInventoryUpdated(AssetInventoryRow::default()),
            RowChange::AssetInventoryCleared {
                host_alias: "h".into(),
                harness: "claude".into(),
            },
        ] {
            assert_eq!(c.name(), pinned(&c));
        }
    }

    /// A new `RowChange` variant whose kind is missing here would be
    /// unfilterable: `?kinds=<it>` would be logged as unrecognised and drop
    /// every one of its events.
    #[test]
    fn event_kinds_cover_every_name() {
        for name in FRONTEND_EVENT_NAMES {
            let kind = name.split_once(':').expect("every name has a kind").0;
            assert!(
                EVENT_KINDS.contains(&kind),
                "EVENT_KINDS is missing {kind} (from {name})"
            );
        }
        for kind in EVENT_KINDS {
            assert!(
                FRONTEND_EVENT_NAMES
                    .iter()
                    .any(|n| n.starts_with(&format!("{kind}:"))),
                "EVENT_KINDS lists {kind}, which no event uses"
            );
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

    #[tokio::test]
    async fn a_subscriber_receives_emitted_row_changes() {
        let bus = BroadcastEventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(42));
        let msg = rx.recv().await.expect("one message");
        assert_eq!(msg.name, "session:killed");
        assert_eq!(msg.payload["id"], 42);
    }

    /// The no-subscriber guard must not cost a live subscriber an event: the
    /// receiver is taken before the emit, which is the only ordering the
    /// stream ever uses (the route subscribes before its first frame).
    #[tokio::test]
    async fn a_live_subscriber_still_gets_everything_after_the_guard() {
        let bus = BroadcastEventBus::new(16);
        assert_eq!(bus.receiver_count(), 0, "nobody is listening yet");
        let mut rx = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        bus.emit(&RowChange::SessionKilled(1));
        bus.emit(&RowChange::HostRemoved("box".into()));
        assert_eq!(rx.recv().await.unwrap().name, "session:killed");
        assert_eq!(rx.recv().await.unwrap().name, "host:removed");
        // A receiver that goes away takes the count with it, and emitting is
        // still fine.
        drop(rx);
        assert_eq!(bus.receiver_count(), 0);
        bus.emit(&RowChange::SessionKilled(2));
    }

    #[tokio::test]
    async fn emitting_without_subscribers_is_not_an_error() {
        let bus = BroadcastEventBus::new(4);
        bus.emit(&RowChange::SessionKilled(1)); // must not panic
    }

    #[tokio::test]
    async fn a_lagging_subscriber_reports_lag_rather_than_stalling_the_bus() {
        let bus = BroadcastEventBus::new(2);
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.emit(&RowChange::SessionKilled(i));
        }
        let err = rx.recv().await.unwrap_err();
        assert!(matches!(
            err,
            tokio::sync::broadcast::error::RecvError::Lagged(_)
        ));
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
