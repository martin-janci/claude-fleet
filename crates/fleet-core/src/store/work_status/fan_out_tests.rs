//! Gap B (2026-09-29): a session's `work.effective_status` reads ANOTHER
//! session's `claude_status` (a working session lifts a local item to
//! `in_progress` for every session on it), so a status writer that moves a
//! row into or out of `working` re-announces the other sessions on its
//! local items (`Store::fan_out_working_change`) — and only on a
//! transition.

use crate::service::pane_intel::ClaudeStatus;
use crate::store::test_support::store_with_recorder;
use crate::store::{HostReconcile, ReconcileSession, Store, WorkTarget};

/// Two live sessions `a` and `b` on host `local`, both on one local item
/// (`a`'s named work, `b` linked to it by a person), both idle.
fn two_on_one_item(s: &Store) -> (i64, i64, i64) {
    s.upsert_host("local").unwrap();
    let reconcile = |names: &[&'static str]| {
        let sessions: Vec<ReconcileSession<'_>> = names
            .iter()
            .map(|n| ReconcileSession {
                tmux_name: n,
                created_at: 1,
                last_activity_at: 1,
                claude_status: Some("idle".into()),
                intel_observed: true,
                ..Default::default()
            })
            .collect();
        let keep: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        s.apply_host_reconcile_in_tx(HostReconcile {
            alias: "local",
            reachable: true,
            last_pinged_at: 1,
            sessions: &sessions,
            keep: &keep,
            ..Default::default()
        })
        .unwrap();
    };
    reconcile(&["a", "b"]);
    let a = s.get_session("a", "local").unwrap().unwrap().id;
    let b = s.get_session("b", "local").unwrap().unwrap().id;
    let (item, _) = s.name_session_work(a, None, "shared work").unwrap();
    s.link_session_work(b, WorkTarget::Item(item.id), "manual")
        .unwrap();
    (a, b, item.id)
}

fn effective(s: &Store, sid: i64) -> Option<String> {
    s.get_session_by_id(sid)
        .unwrap()
        .unwrap()
        .work
        .and_then(|w| w.effective_status)
}

fn row_version(s: &Store, sid: i64) -> i64 {
    s.get_session_by_id(sid).unwrap().unwrap().row_version
}

#[test]
fn a_hook_turning_one_session_working_re_announces_the_other() {
    let (s, bus) = store_with_recorder();
    let (a, b, _) = two_on_one_item(&s);
    assert_eq!(effective(&s, b).as_deref(), Some("todo"));
    let v0 = row_version(&s, b);
    bus.take();

    s.record_notification_hook_for_row(a, ClaudeStatus::Working, None)
        .unwrap();
    let events = bus.take();
    assert!(
        events.contains(&format!("session:updated:{b}")),
        "b's row reads a's status: {events:?}"
    );
    assert!(row_version(&s, b) > v0, "b's row_version moves with it");
    assert_eq!(effective(&s, b).as_deref(), Some("in_progress"));

    // Still working: no transition, nothing for b.
    let v1 = row_version(&s, b);
    s.record_notification_hook_for_row(a, ClaudeStatus::Working, None)
        .unwrap();
    let events = bus.take();
    assert!(
        !events.contains(&format!("session:updated:{b}")),
        "no transition, no fan-out: {events:?}"
    );
    assert_eq!(row_version(&s, b), v1);

    // The turn ends: b is re-announced and no longer lifted.
    s.record_stop_hook_for_row(a).unwrap();
    let events = bus.take();
    assert!(
        events.contains(&format!("session:updated:{b}")),
        "b re-announced when a stops: {events:?}"
    );
    assert!(row_version(&s, b) > v1);
    assert_eq!(effective(&s, b).as_deref(), Some("todo"));
}

#[test]
fn a_reconcile_pass_turning_one_session_working_re_announces_the_other() {
    let (s, bus) = store_with_recorder();
    let (_a, b, _) = two_on_one_item(&s);
    let pass = |a_status: &str| {
        let sessions = [
            ReconcileSession {
                tmux_name: "a",
                created_at: 1,
                last_activity_at: 1,
                claude_status: Some(a_status.into()),
                intel_observed: true,
                ..Default::default()
            },
            ReconcileSession {
                tmux_name: "b",
                created_at: 1,
                last_activity_at: 1,
                claude_status: Some("idle".into()),
                intel_observed: true,
                ..Default::default()
            },
        ];
        let keep = vec!["a".to_string(), "b".to_string()];
        s.apply_host_reconcile_in_tx(HostReconcile {
            alias: "local",
            reachable: true,
            last_pinged_at: 2,
            sessions: &sessions,
            keep: &keep,
            ..Default::default()
        })
        .unwrap();
    };
    bus.take();
    pass("working");
    let events = bus.take();
    assert!(
        events.contains(&format!("session:updated:{b}")),
        "{events:?}"
    );
    assert_eq!(effective(&s, b).as_deref(), Some("in_progress"));

    pass("idle");
    let events = bus.take();
    assert!(
        events.contains(&format!("session:updated:{b}")),
        "{events:?}"
    );
    assert_eq!(effective(&s, b).as_deref(), Some("todo"));
}

/// A person's status is final: a working session changes nothing there,
/// so nothing is re-announced.
#[test]
fn a_person_set_status_is_not_fanned_out() {
    let (s, bus) = store_with_recorder();
    let (a, b, item) = two_on_one_item(&s);
    s.set_item_status(item, "done").unwrap();
    bus.take();
    s.record_notification_hook_for_row(a, ClaudeStatus::Working, None)
        .unwrap();
    let events = bus.take();
    assert!(
        !events.contains(&format!("session:updated:{b}")),
        "{events:?}"
    );
    assert_eq!(effective(&s, b).as_deref(), Some("done"));
}
