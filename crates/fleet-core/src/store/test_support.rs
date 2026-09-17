//! Test helpers shared by the `store` test modules.

use super::*;

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
