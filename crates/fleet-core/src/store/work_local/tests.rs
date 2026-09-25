//! Local work items (work graph M11.1): naming, renaming, listing.

use super::*;
use crate::events::RecordingEventBus;
use crate::store::{TrackerItemWrite, WorkTarget};
use std::sync::Arc;

fn seed(s: &Store, name: &str) -> i64 {
    s.upsert_host("h").unwrap();
    s.upsert_session(name, "h", None, None, 1, 1, "running", None)
        .unwrap()
}

fn with_bus() -> (Store, Arc<RecordingEventBus>) {
    let bus = Arc::new(RecordingEventBus::new());
    let dyn_bus: Arc<dyn crate::events::EventBus> = bus.clone();
    (Store::open_with_bus_in_memory(dyn_bus).unwrap(), bus)
}

#[test]
fn a_title_is_trimmed_bounded_and_printable() {
    assert_eq!(validate_local_work_title("  Billing  ").unwrap(), "Billing");
    for bad in ["", "   ", "a\u{7}b", "line\nbreak"] {
        assert_eq!(
            validate_local_work_title(bad).unwrap_err().code,
            codes::E_INVALID,
            "{bad:?}"
        );
    }
    let max = "x".repeat(LOCAL_WORK_TITLE_MAX_CHARS);
    assert!(validate_local_work_title(&max).is_ok());
    // Characters, not bytes.
    assert!(validate_local_work_title(&"é".repeat(LOCAL_WORK_TITLE_MAX_CHARS)).is_ok());
    assert_eq!(
        validate_local_work_title(&format!("{max}x"))
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

#[test]
fn naming_links_a_new_local_item_as_primary_work() {
    let (s, bus) = with_bus();
    let sid = seed(&s, "dev");
    s.set_claude_session_id(sid, "conv-1").unwrap();
    bus.take();
    let (item, link) = s
        .name_session_work(sid, Some("BILLING"), " Billing cleanup ")
        .unwrap();
    assert_eq!(item.source, "local");
    assert_eq!(item.key.as_deref(), Some("BILLING"));
    assert_eq!(item.title, "Billing cleanup");
    assert_eq!(link.item_id, Some(item.id));
    assert_eq!(link.ref_key, None);
    assert_eq!(link.state, "confirmed");
    assert_eq!(link.source, "manual");
    assert_eq!(link.strength.as_deref(), Some("explicit"));
    assert_eq!(link.claude_session_id.as_deref(), Some("conv-1"));
    assert!(link.is_primary, "no other primary: this one is");
    let w = s.get_session_by_id(sid).unwrap().unwrap().work.unwrap();
    assert_eq!(
        (w.item_id, w.title.as_str()),
        (Some(item.id), "Billing cleanup")
    );
    assert_eq!(
        bus.names(),
        vec!["session:updated", "work:item"],
        "the row, then the item"
    );
}

#[test]
fn naming_keeps_an_existing_primary() {
    let s = Store::open_in_memory().unwrap();
    let sid = seed(&s, "dev");
    let first = s
        .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    let (_, named) = s.name_session_work(sid, None, "Side quest").unwrap();
    assert!(!named.is_primary, "the session already had primary work");
    let links = s.session_work_links(sid).unwrap();
    assert!(links.iter().any(|l| l.id == first.id && l.is_primary));
    assert_eq!(
        s.get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .work
            .unwrap()
            .link_id,
        first.id
    );
}

#[test]
fn a_taken_local_key_is_refused_and_a_keyless_name_is_always_new() {
    let s = Store::open_in_memory().unwrap();
    let a = seed(&s, "a");
    let b = seed(&s, "b");
    s.name_session_work(a, Some("OPS"), "Ops").unwrap();
    assert_eq!(
        s.name_session_work(b, Some("OPS"), "Other")
            .unwrap_err()
            .code,
        codes::E_EXISTS
    );
    let (x, _) = s.name_session_work(a, None, "Same").unwrap();
    let (y, _) = s.name_session_work(b, None, "Same").unwrap();
    assert_ne!(x.id, y.id);
    assert_eq!(
        s.name_session_work(9_999, None, "Ghost").unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        s.name_session_work(a, None, "  ").unwrap_err().code,
        codes::E_INVALID
    );
}

#[test]
fn renaming_touches_local_items_only_and_announces_the_rows_that_show_it() {
    let (s, bus) = with_bus();
    let sid = seed(&s, "dev");
    let (item, _) = s.name_session_work(sid, None, "Old").unwrap();
    bus.take();
    let renamed = s.rename_local_work_item(item.id, "New").unwrap().unwrap();
    assert_eq!(renamed.title, "New");
    assert_eq!(
        s.get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .work
            .unwrap()
            .title,
        "New"
    );
    assert_eq!(bus.names(), vec!["work:item", "session:updated"]);
    // The same title again changes nothing and says nothing.
    bus.take();
    s.rename_local_work_item(item.id, "New").unwrap().unwrap();
    assert!(bus.names().is_empty());

    let t = s
        .add_tracker("jira", "J", "https://x.atlassian.net")
        .unwrap();
    let ticket = s
        .upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some("TT-1".into()),
                title: "Ticket".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    assert_eq!(s.rename_local_work_item(ticket, "Mine").unwrap(), None);
    assert_eq!(s.get_work_item(ticket).unwrap().unwrap().title, "Ticket");
    assert_eq!(s.rename_local_work_item(9_999, "X").unwrap(), None);
    assert_eq!(
        s.rename_local_work_item(item.id, "bad\u{0}")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
}

#[test]
fn local_item_links_name_the_live_session_and_its_host() {
    let s = Store::open_in_memory().unwrap();
    let a = seed(&s, "a");
    let b = seed(&s, "b");
    let (item, _) = s.name_session_work(a, Some("LX-1"), "Lx").unwrap();
    s.link_session_work(b, WorkTarget::Item(item.id), "manual")
        .unwrap();
    s.name_session_work(a, None, "Other").unwrap();
    let links = s.local_item_links(Some(item.id)).unwrap();
    assert_eq!(links.len(), 2);
    assert!(links
        .iter()
        .all(|l| l.item_id == item.id && l.session_host.as_deref() == Some("h")));
    s.delete_session(b).unwrap();
    let links = s.local_item_links(Some(item.id)).unwrap();
    let ended: Vec<_> = links.iter().filter(|l| l.link.ended_at.is_some()).collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        ended[0].session_id, None,
        "an ended link has no live session"
    );
    assert_eq!(s.local_item_links(None).unwrap().len(), 3);
    assert_eq!(s.local_work_items().unwrap().len(), 2);
}
