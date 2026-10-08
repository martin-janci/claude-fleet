//! Edges and holds (orchestration O2): a cycle is refused, an org is never
//! crossed, a mission's member logs what changed.

use super::*;
use crate::store::NewMission;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn local(s: &Store, title: &str) -> i64 {
    s.create_local_work_item(None, title).unwrap().id
}

#[test]
fn an_edge_is_drawn_once_and_erased() {
    let s = store();
    let (a, b) = (local(&s, "a"), local(&s, "b"));
    assert!(s.add_item_dep(a, b, "person", "person:1").unwrap());
    assert!(
        !s.add_item_dep(a, b, "person", "person:1").unwrap(),
        "already there"
    );
    let e = s.item_deps(&[a]).unwrap();
    assert_eq!(
        (e.len(), e[0].depends_on, e[0].kind.as_str()),
        (1, b, "blocks")
    );
    assert!(
        s.item_deps(&[b]).unwrap().is_empty(),
        "edges go out of the waiting item"
    );
    assert!(s.remove_item_dep(a, b, "person:1").unwrap());
    assert!(!s.remove_item_dep(a, b, "person:1").unwrap());
}

#[test]
fn a_cycle_is_refused_however_long() {
    let s = store();
    let ids: Vec<i64> = (0..5).map(|i| local(&s, &format!("t{i}"))).collect();
    for w in ids.windows(2) {
        s.add_item_dep(w[1], w[0], "person", "fleet").unwrap();
    }
    let e = s
        .add_item_dep(ids[0], ids[4], "person", "fleet")
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("cycle"), "{}", e.message);
    assert_eq!(
        s.add_item_dep(ids[0], ids[0], "person", "fleet")
            .unwrap_err()
            .code,
        codes::E_INVALID
    );
    assert_eq!(
        s.item_deps(&[ids[0]]).unwrap().len(),
        0,
        "nothing was written"
    );
}

#[test]
fn an_edge_stays_inside_one_org() {
    let s = store();
    let org = s.add_org("Acme", None, false).unwrap().id;
    let (a, b) = (local(&s, "a"), local(&s, "b"));
    s.set_local_item_org(a, Some(org)).unwrap();
    assert_eq!(
        s.add_item_dep(a, b, "person", "fleet").unwrap_err().code,
        codes::E_FORBIDDEN
    );
    s.set_local_item_org(b, Some(org)).unwrap();
    assert!(s.add_item_dep(a, b, "person", "fleet").unwrap());
}

#[test]
fn an_unknown_source_or_item_is_refused() {
    let s = store();
    let a = local(&s, "a");
    assert_eq!(
        s.add_item_dep(a, 999_999, "person", "fleet")
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
    let b = local(&s, "b");
    assert_eq!(
        s.add_item_dep(a, b, "robot", "fleet").unwrap_err().code,
        codes::E_INVALID
    );
}

#[test]
fn a_hold_is_set_once_and_a_member_logs_it() {
    let s = store();
    let root = local(&s, "epic");
    let m = s
        .create_mission(
            &NewMission {
                name: "m",
                goal: "g",
                root_item_id: Some(root),
                ..Default::default()
            },
            "person:1",
        )
        .unwrap();
    let other = local(&s, "other");
    assert!(s.set_item_hold(root, true, "person:1").unwrap());
    assert!(!s.set_item_hold(root, true, "person:1").unwrap());
    assert!(s.get_work_item(root).unwrap().unwrap().held_at.is_some());
    s.add_item_dep(root, other, "person", "person:1").unwrap();
    assert!(s.set_item_hold(root, false, "person:1").unwrap());
    let kinds: Vec<String> = s
        .mission_events(m.id, None, 10)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, ["released", "dep_added", "held", "created"]);
    assert_eq!(
        s.set_item_hold(999_999, true, "x").unwrap_err().code,
        codes::E_NOTFOUND
    );
}

#[test]
fn deleting_an_item_takes_its_edges() {
    let s = store();
    let (a, b) = (local(&s, "a"), local(&s, "b"));
    s.add_item_dep(a, b, "person", "fleet").unwrap();
    s.conn
        .execute("DELETE FROM work_items WHERE id = ?1", [b])
        .unwrap();
    assert!(s.item_deps(&[a]).unwrap().is_empty());
}
