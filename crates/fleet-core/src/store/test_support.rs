//! Test helpers shared by the `store` test modules.

use super::*;

/// Make the next COMMIT on `s` fail WITHOUT SQLite rolling anything back,
/// once `event` (`"INSERT ON session_events"`, `"UPDATE ON sessions"`, …)
/// fires: a TEMP trigger inserts an orphan row under a DEFERRED foreign key,
/// which SQLite checks only when the outermost transaction commits. A COMMIT
/// — or a top-level `RELEASE`, which is one — refused that way leaves the
/// transaction open, the state no SAVEPOINT helper may leave behind.
pub(super) fn arm_commit_failure(s: &Store, event: &str) {
    s.conn
        .execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS fk_parent (id INTEGER PRIMARY KEY);
             CREATE TEMP TABLE IF NOT EXISTS fk_orphan (
                 parent INTEGER REFERENCES fk_parent(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TEMP TRIGGER arm_commit_failure AFTER {event}
             BEGIN INSERT INTO fk_orphan (parent) VALUES (999); END;"
        ))
        .unwrap();
}

pub(super) fn store_with_recorder() -> (Store, Arc<crate::events::RecordingEventBus>) {
    let bus = Arc::new(crate::events::RecordingEventBus::new());
    let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
    (store, bus)
}

/// A reachable probe of `alias` at `ts` that saw no sessions and no
/// versions, with the stale-probe guard off — the shape most reconcile
/// tests apply; spread the fields that matter over it.
pub(super) fn empty_probe(alias: &str, ts: i64) -> HostReconcile<'_> {
    HostReconcile {
        alias,
        reachable: true,
        last_pinged_at: ts,
        ..Default::default()
    }
}

pub(super) fn reconcile_one(
    s: &mut Store,
    name: &'static str,
    status: Option<&str>,
    stuck: Option<&str>,
    pr: Option<(Option<&str>, Option<&str>)>,
) -> SessionRow {
    let (pr_url, ci_status, pr_observed) = match pr {
        Some((u, c)) => (u.map(String::from), c.map(String::from), true),
        None => (None, None, false),
    };
    s.apply_host_reconcile(HostReconcile {
        sessions: &[ReconcileSession {
            tmux_name: name,
            created_at: 1,
            last_activity_at: 1,
            claude_status: status.map(String::from),
            pr_url,
            stuck_kind: stuck.map(String::from),
            intel_observed: true,
            ci_status,
            pr_observed,
            ..Default::default()
        }],
        keep: &[name.to_string()],
        ..empty_probe("local", 1)
    })
    .unwrap();
    s.get_session(name, "local").unwrap().unwrap()
}

/// A tracker item `key` (external id `ext`) of `tracker_id`, status todo.
pub(crate) fn tracker_item(s: &Store, tracker_id: i64, ext: &str, key: &str, title: &str) -> i64 {
    s.upsert_tracker_item(
        tracker_id,
        &TrackerItemWrite {
            external_id: ext.into(),
            key: Some(key.into()),
            title: title.into(),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            ..Default::default()
        },
    )
    .expect("tracker item")
    .id
}
