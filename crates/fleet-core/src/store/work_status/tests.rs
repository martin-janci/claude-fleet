//! `Store::set_item_status`: a person's status for local work, refused for a
//! tracker's ticket (design 2026-09-28 §2).

use super::*;
use crate::store::TrackerItemWrite;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

/// A new local work item, with no session link — enough to carry a status.
fn name_local_item(s: &Store, title: &str) -> i64 {
    s.create_local_work_item(None, title).unwrap().id
}

/// One tracker item under a fresh tracker, keyed `key`.
fn seed_tracker_item(s: &Store, key: &str) -> i64 {
    let t = s
        .add_tracker("jira", key, "https://acme.atlassian.net")
        .unwrap();
    s.upsert_tracker_item(
        t.id,
        &TrackerItemWrite {
            external_id: "1".into(),
            key: Some(key.into()),
            title: format!("{key} ticket"),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

#[test]
fn a_person_can_set_a_local_items_status() {
    let s = store();
    let id = name_local_item(&s, "auth refactor");
    let row = s.set_item_status(id, "in_progress").unwrap().unwrap();
    assert_eq!(row.status_category, "in_progress");
    assert_eq!(row.status_set_by.as_deref(), Some("person"));
    assert!(row.status_set_at.is_some());
}

#[test]
fn a_tracker_items_status_belongs_to_its_tracker() {
    let s = store();
    let id = seed_tracker_item(&s, "ABC-1");
    let e = s.set_item_status(id, "done").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        e.message.contains("ABC-1"),
        "the refusal must name the ticket: {}",
        e.message
    );
}

#[test]
fn only_the_three_categories_are_accepted() {
    let s = store();
    let id = name_local_item(&s, "x");
    for bad in ["blocked", "review", "", "DONE"] {
        let e = s.set_item_status(id, bad).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "accepted {bad:?}");
    }
}

#[test]
fn an_unknown_id_is_not_an_error() {
    let s = store();
    assert!(s.set_item_status(9_999, "done").unwrap().is_none());
}

/// A tracker item with no key still gets named in its refusal — by title,
/// since `key.unwrap_or_else(|| title)` has no keyed fallback to lean on.
#[test]
fn a_keyless_tracker_items_refusal_names_it_by_title() {
    let s = store();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    let item = s
        .upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: None,
                title: "untitled ticket".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let e = s.set_item_status(item, "done").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        e.message.contains("untitled ticket"),
        "the refusal must name the ticket by title: {}",
        e.message
    );
}

#[test]
fn a_merged_pr_stamps_a_local_item_done() {
    let s = store();
    let id = name_local_item(&s, "auth refactor");
    assert!(s.stamp_derived_done(id).unwrap());
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "done");
    assert_eq!(row.status_set_by.as_deref(), Some("derived"));
}

#[test]
fn a_persons_status_outranks_the_stamp() {
    let s = store();
    let id = name_local_item(&s, "auth refactor");
    s.set_item_status(id, "in_progress").unwrap();
    assert!(
        !s.stamp_derived_done(id).unwrap(),
        "the stamp must not write"
    );
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "in_progress");
    assert_eq!(row.status_set_by.as_deref(), Some("person"));
}

#[test]
fn the_stamp_never_touches_a_tracker_item() {
    let s = store();
    let id = seed_tracker_item(&s, "ABC-1");
    assert!(!s.stamp_derived_done(id).unwrap());
}

#[test]
fn stamping_twice_writes_once() {
    let s = store();
    let id = name_local_item(&s, "x");
    assert!(s.stamp_derived_done(id).unwrap());
    assert!(!s.stamp_derived_done(id).unwrap(), "already done, no event");
}

/// A session with local work and a merged PR: the stamp follows the PRIMARY
/// confirmed link and nothing else. `set_pr_signals` — the write that
/// records the merged fact, and the site the background pass reaches — is
/// what drives it here; no tidy pass is involved (see
/// `service::work::tidy::tests::a_merged_pr_stamps_done_in_the_background_with_auto_tidy_off`
/// for the same claim against the real GC tick).
#[test]
fn a_merged_pr_stamps_the_sessions_primary_work_only() {
    let s = store();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    // The primary link (the first one a session gets), then a secondary
    // confirmed link to other local work: "my PR merged" says nothing about
    // the second one — an epic must not go `done` because a child's PR did.
    let (primary, _) = s
        .name_session_work(sid, Some("PRIM-1"), "the work")
        .unwrap();
    let (secondary, _) = s.name_session_work(sid, Some("SEC-1"), "an epic").unwrap();

    let merged = serde_json::json!({ "state": "MERGED" }).to_string();
    assert_eq!(
        s.set_pr_signals("h", "dev", Some(&merged)).unwrap(),
        Some(sid)
    );

    let p = s.get_work_item(primary.id).unwrap().unwrap();
    assert_eq!(p.status_category, "done");
    assert_eq!(p.status_set_by.as_deref(), Some("derived"));
    let sec = s.get_work_item(secondary.id).unwrap().unwrap();
    assert_eq!(
        sec.status_category, "todo",
        "a secondary link is not the PR's work"
    );
    assert_eq!(sec.status_set_by, None);
}

/// An OPEN PR stamps nothing, and a merged one stamps at most once — the
/// probe calls `set_pr_signals` on every pass.
#[test]
fn only_a_merged_pr_stamps_and_only_once() {
    let s = store();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let (item, _) = s.name_session_work(sid, None, "the work").unwrap();
    let open = serde_json::json!({ "state": "OPEN" }).to_string();
    s.set_pr_signals("h", "dev", Some(&open)).unwrap();
    assert_eq!(
        s.get_work_item(item.id).unwrap().unwrap().status_category,
        "todo"
    );
    let merged = serde_json::json!({ "state": "MERGED" }).to_string();
    s.set_pr_signals("h", "dev", Some(&merged)).unwrap();
    let first = s.get_work_item(item.id).unwrap().unwrap();
    assert_eq!(first.status_category, "done");
    // Same signals again: the UPDATE matches nothing, so the stamp (and its
    // `status_changed_at`) stands where it was.
    s.set_pr_signals("h", "dev", Some(&merged)).unwrap();
    let second = s.get_work_item(item.id).unwrap().unwrap();
    assert_eq!(second.status_set_at, first.status_set_at);
    assert_eq!(second.status_changed_at, first.status_changed_at);
}

/// A bare `ref_key` link has no item to stamp, and a session with no link
/// at all is not an error.
#[test]
fn a_merged_pr_with_no_linked_item_stamps_nothing() {
    let s = store();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    assert_eq!(s.stamp_derived_done_for_session(sid).unwrap(), 0);
    s.link_session_work(sid, crate::store::WorkTarget::Key("BARE-1"), "manual")
        .unwrap();
    assert_eq!(s.stamp_derived_done_for_session(sid).unwrap(), 0);
}
