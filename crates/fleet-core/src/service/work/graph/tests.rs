//! The derived graph (orchestration O2): each node's state and wave, the
//! phase it gives an active mission, and the fences on its writes.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::service::work::missions::{self, MissionInput};
use crate::store::{MissionRow, TreeEntry, TreeRef};
use std::sync::Mutex;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory().unwrap())
}

fn person(store: &Mutex<Store>, id: i64) -> ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(id),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

/// Ana's mission with three members besides the root.
fn fixture() -> (Mutex<Store>, ViewScope, MissionRow, [i64; 3]) {
    let st = store();
    let ana = lock(&st).unwrap().create_person("ana", None).unwrap().id;
    let me = person(&st, ana);
    let m = missions::save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("m".into()),
                goal: Some("g".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &st,
        &me,
    )
    .unwrap();
    let ids = {
        let s = lock(&st).unwrap();
        let mut ids = [0; 3];
        for (k, t) in ["schema", "api", "ui"].iter().enumerate() {
            ids[k] = s.create_local_work_item(None, t).unwrap().id;
            s.set_mission_item(m.id, ids[k], true, "person:1").unwrap();
        }
        ids
    };
    (st, me, m, ids)
}

fn dep_args(item: i64, on: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "dep".into(),
        item_id: Some(item),
        depends_on: Some(on),
        ..Default::default()
    }
}

fn graph(st: &Mutex<Store>, me: &ViewScope, m: i64) -> missions::MissionDetail {
    missions::mission(st, me, m, None).unwrap()
}

fn state_of(d: &missions::MissionDetail, id: i64) -> (String, u32) {
    let n = d.graph.nodes.iter().find(|n| n.item_id == id).unwrap();
    (n.state.clone(), n.wave)
}

#[test]
fn a_chain_makes_waves_and_only_its_head_is_ready() {
    let (st, me, m, [a, b, c]) = fixture();
    dep(&dep_args(b, a), &st, &me).unwrap();
    dep(&dep_args(c, b), &st, &me).unwrap();
    dep(&dep_args(c, a), &st, &me).unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, a), ("ready".into(), 1));
    assert_eq!(state_of(&d, b), ("waiting".into(), 2));
    assert_eq!(state_of(&d, c), ("waiting".into(), 3));
    assert_eq!(d.graph.waves, 3);
    lock(&st).unwrap().set_item_status(a, "done").unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, a).0, "done");
    assert_eq!(state_of(&d, b).0, "ready");
    let c_node = d.graph.nodes.iter().find(|n| n.item_id == c).unwrap();
    assert_eq!(c_node.waiting_for, vec![b], "a is done, b is not");
}

#[test]
fn running_failed_held_and_blocked_are_told_apart() {
    let (st, me, m, [a, b, c]) = fixture();
    dep(&dep_args(b, a), &st, &me).unwrap();
    {
        let s = lock(&st).unwrap();
        let t = s.insert_task(None, None, "go", "n1").unwrap();
        s.set_task_run(t.id, a, 1, "implement").unwrap();
    }
    missions::set_state(
        &WorkLinkArgs {
            action: "mission_state".into(),
            mission_id: Some(m.id),
            status: Some("active".into()),
            ..Default::default()
        },
        &st,
        &me,
    )
    .unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, a).0, "running");
    assert_eq!(d.phase.as_deref(), Some("running"));
    {
        let s = lock(&st).unwrap();
        let t = s.tasks_for_item(a).unwrap()[0].id;
        s.finish_task(t, "failed", None, Some("boom")).unwrap();
    }
    hold(
        &WorkLinkArgs {
            action: "hold".into(),
            item_id: Some(c),
            ..Default::default()
        },
        &st,
        &me,
    )
    .unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, a).0, "failed");
    assert_eq!(state_of(&d, b).0, "blocked", "it waits for a failed item");
    assert_eq!(state_of(&d, c).0, "held");
    // The root is still ready, so the mission waits rather than blocks.
    assert_eq!(d.phase.as_deref(), Some("waiting"));
}

#[test]
fn an_item_outside_the_mission_blocks_and_is_named() {
    let (st, me, m, [a, ..]) = fixture();
    let outside = lock(&st)
        .unwrap()
        .create_local_work_item(None, "ticket")
        .unwrap()
        .id;
    dep(&dep_args(a, outside), &st, &me).unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, a).0, "blocked");
    assert_eq!(d.graph.outside.len(), 1);
    assert_eq!(d.graph.outside[0].title, "ticket");
}

#[test]
fn a_proposed_tree_is_not_ready_until_accepted_and_undo_takes_it_back() {
    let (st, me, m, _) = fixture();
    let root = m.root_item_id.unwrap();
    let made = lock(&st)
        .unwrap()
        .propose_tree(
            root,
            &[
                TreeEntry {
                    title: "plan a".into(),
                    ..Default::default()
                },
                TreeEntry {
                    title: "plan b".into(),
                    depends_on: vec![TreeRef::Entry(0)],
                    ..Default::default()
                },
            ],
            "agent",
        )
        .unwrap();
    let ids: Vec<i64> = made.iter().map(|i| i.id).collect();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, ids[0]).0, "proposed");
    let args = WorkLinkArgs {
        action: "accept_many".into(),
        item_ids: Some(ids.clone()),
        ..Default::default()
    };
    accept_many(&args, &st, &me).unwrap();
    let d = graph(&st, &me, m.id);
    assert_eq!(state_of(&d, ids[0]).0, "ready");
    assert_eq!(state_of(&d, ids[1]), ("waiting".into(), 2));
    undo_accept(&args, &st, &me).unwrap();
    assert_eq!(state_of(&graph(&st, &me, m.id), ids[1]).0, "proposed");
}

#[test]
fn another_person_cannot_draw_on_a_mission_they_cannot_see() {
    let (st, _, _, [a, b, _]) = fixture();
    let bo = lock(&st).unwrap().create_person("bo", None).unwrap().id;
    let bo = person(&st, bo);
    let e = dep(&dep_args(b, a), &st, &bo).unwrap_err();
    let unknown = dep(&dep_args(999_999, a), &st, &bo).unwrap_err();
    assert_eq!(e.code, unknown.code);
    let accept = WorkLinkArgs {
        action: "accept_many".into(),
        item_ids: Some(vec![a]),
        ..Default::default()
    };
    assert_eq!(
        accept_many(&accept, &st, &bo).unwrap_err().code,
        unknown.code
    );
}

#[test]
fn a_scoped_caller_never_accepts() {
    let (st, _, _, [a, ..]) = fixture();
    let host = Caller {
        api: None,
        host_alias: Some("h".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(&st).unwrap())
    .unwrap();
    let args = WorkLinkArgs {
        action: "accept_many".into(),
        item_ids: Some(vec![a]),
        ..Default::default()
    };
    assert_eq!(
        accept_many(&args, &st, &host).unwrap_err().code,
        codes::E_FORBIDDEN
    );
}
