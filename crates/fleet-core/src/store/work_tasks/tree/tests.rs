//! A proposed tree (orchestration O2): all or nothing, its edges, its
//! mission, and accepting it at once and taking that back.

use super::*;
use crate::store::NewMission;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn entry(title: &str, deps: &[TreeRef]) -> TreeEntry {
    TreeEntry {
        title: title.into(),
        depends_on: deps.to_vec(),
        ..Default::default()
    }
}

fn root_in_mission(s: &Store) -> (i64, i64) {
    let root = s.create_local_work_item(None, "epic").unwrap().id;
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
    (root, m.id)
}

#[test]
fn a_tree_lands_with_its_edges_in_its_mission() {
    let s = store();
    let (root, m) = root_in_mission(&s);
    let made = s
        .propose_tree(
            root,
            &[
                entry("schema", &[]),
                entry("api", &[TreeRef::Entry(0)]),
                entry("ui", &[TreeRef::Entry(1), TreeRef::Entry(0)]),
            ],
            "agent",
        )
        .unwrap();
    assert_eq!(made.len(), 3);
    assert!(made
        .iter()
        .all(|i| i.proposal_state.as_deref() == Some("proposed")));
    let deps = s.item_deps(&[made[2].id]).unwrap();
    assert_eq!(deps.len(), 2);
    assert!(deps.iter().all(|d| d.source == "proposal"));
    assert_eq!(
        s.mission_items(m).unwrap().len(),
        4,
        "the root and three proposals"
    );
}

#[test]
fn a_bad_tree_writes_nothing() {
    let s = store();
    let (root, m) = root_in_mission(&s);
    let forward = s
        .propose_tree(
            root,
            &[entry("a", &[TreeRef::Entry(1)]), entry("b", &[])],
            "agent",
        )
        .unwrap_err();
    assert_eq!(forward.code, codes::E_INVALID);
    let missing = s
        .propose_tree(
            root,
            &[entry("a", &[]), entry("b", &[TreeRef::Item(999_999)])],
            "agent",
        )
        .unwrap_err();
    assert_eq!(missing.code, codes::E_NOTFOUND);
    let empty_title = s.propose_tree(root, &[entry("a", &[]), entry("  ", &[])], "agent");
    assert!(empty_title.is_err());
    let too_many: Vec<TreeEntry> = (0..=PROPOSALS_OPEN_CAP)
        .map(|i| entry(&format!("t{i}"), &[]))
        .collect();
    assert_eq!(
        s.propose_tree(root, &too_many, "agent").unwrap_err().code,
        codes::E_LIMIT
    );
    assert_eq!(s.native_children(root).unwrap().len(), 0);
    assert_eq!(s.mission_items(m).unwrap().len(), 1);
}

#[test]
fn many_are_accepted_together_or_not_at_all_and_undone() {
    let s = store();
    let (root, _) = root_in_mission(&s);
    let made = s
        .propose_tree(
            root,
            &[entry("a", &[]), entry("b", &[TreeRef::Entry(0)])],
            "agent",
        )
        .unwrap();
    let ids: Vec<i64> = made.iter().map(|i| i.id).collect();
    let e = s.accept_proposals(&[ids[0], root]).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID, "the root is no proposal");
    assert_eq!(
        s.get_work_item(ids[0])
            .unwrap()
            .unwrap()
            .proposal_state
            .as_deref(),
        Some("proposed"),
        "the refusal took the first accept back"
    );
    let accepted = s.accept_proposals(&ids).unwrap();
    assert!(accepted
        .iter()
        .all(|i| i.proposal_state.as_deref() == Some("accepted")));
    let undone = s.undo_accept(&ids).unwrap();
    assert!(undone
        .iter()
        .all(|i| i.proposal_state.as_deref() == Some("proposed")));
}

#[test]
fn an_accept_that_was_worked_on_cannot_be_undone() {
    let s = store();
    let (root, _) = root_in_mission(&s);
    let made = s.propose_tree(root, &[entry("a", &[])], "agent").unwrap();
    let id = made[0].id;
    s.accept_proposals(&[id]).unwrap();
    let t = s.insert_task(None, None, "go", "n1").unwrap();
    s.set_task_run(t.id, id, 1, "implement").unwrap();
    assert_eq!(
        s.undo_accept(&[id]).unwrap_err().code,
        codes::E_INVALID_STATE
    );
}

#[test]
fn a_full_mission_takes_no_proposal() {
    let s = store();
    let (root, m) = root_in_mission(&s);
    for i in 0..(crate::store::MISSION_ITEM_CAP - 1) {
        let t = s.create_local_work_item(None, &format!("t{i}")).unwrap().id;
        s.set_mission_item(m, t, true, "person:1").unwrap();
    }
    let e = s
        .propose_subtask(&Proposal {
            parent_id: root,
            title: "one more",
            notes: None,
            why: None,
            proposed_by: "agent",
        })
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        s.native_children(root).unwrap().is_empty(),
        "refused before it was written"
    );
}
