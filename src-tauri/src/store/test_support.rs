//! Test helpers shared by the `store` test modules.

use super::*;

pub(super) fn store_with_recorder() -> (Store, Arc<crate::events::RecordingEventBus>) {
    let bus = Arc::new(crate::events::RecordingEventBus::new());
    let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
    (store, bus)
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
        alias: "local",
        reachable: true,
        claude_version: None,
        tmux_version: None,
        last_pinged_at: 1,
        probe_started_at: 0,
        sessions: &[ReconcileSession {
            tmux_name: name,
            project_id: None,
            created_at: 1,
            last_activity_at: 1,
            account_uuid: None,
            worktree_key: None,
            claude_session_id: None,
            claude_status: status.map(String::from),
            effort_level: None,
            pr_url,
            current_activity: None,
            context_pct: None,
            stuck_kind: stuck.map(String::from),
            intel_observed: true,
            ci_status,
            pr_observed,
        }],
        keep: &[name.to_string()],
    })
    .unwrap();
    s.get_session(name, "local").unwrap().unwrap()
}
