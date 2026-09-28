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
