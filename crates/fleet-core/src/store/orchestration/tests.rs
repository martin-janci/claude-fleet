//! Missions in the store (orchestration O1): the lifecycle, one mission
//! per item, the item cap, the repo allow-list and the capped event log.

use super::*;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn local(s: &Store, title: &str) -> i64 {
    s.create_local_work_item(None, title).unwrap().id
}

fn mission(s: &Store, name: &str, root: Option<i64>) -> MissionRow {
    s.create_mission(
        &NewMission {
            name,
            goal: "ship it",
            root_item_id: root,
            ..Default::default()
        },
        "person:1",
    )
    .unwrap()
}

fn kinds(s: &Store, id: i64) -> Vec<String> {
    s.mission_events(id, None, 100)
        .unwrap()
        .into_iter()
        .rev()
        .map(|e| e.kind)
        .collect()
}

#[test]
fn a_new_mission_is_a_draft_that_holds_its_root() {
    let s = store();
    let root = local(&s, "epic");
    let m = mission(&s, "Payments v2", Some(root));
    assert_eq!(m.state, "draft");
    assert_eq!(m.mode, "finite");
    assert_eq!(m.level, 0);
    assert_eq!(m.total, 1);
    assert_eq!(s.item_mission(root).unwrap(), Some(m.id));
    assert_eq!(kinds(&s, m.id), ["created"]);
}

#[test]
fn a_mission_needs_a_name_and_a_goal() {
    let s = store();
    for (name, goal) in [("", "g"), ("n", " "), ("a\nb", "g")] {
        let e = s
            .create_mission(
                &NewMission {
                    name,
                    goal,
                    ..Default::default()
                },
                "fleet",
            )
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{name:?} {goal:?}");
    }
    let e = s
        .create_mission(
            &NewMission {
                name: "n",
                goal: "g",
                level: Some(4),
                ..Default::default()
            },
            "fleet",
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn one_item_roots_one_mission() {
    let s = store();
    let root = local(&s, "epic");
    mission(&s, "a", Some(root));
    let e = s
        .create_mission(
            &NewMission {
                name: "b",
                goal: "g",
                root_item_id: Some(root),
                ..Default::default()
            },
            "fleet",
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
}

#[test]
fn the_lifecycle_moves_only_forward_or_between_active_and_paused() {
    let s = store();
    let m = mission(&s, "m", None);
    let e = s
        .set_mission_state(m.id, None, "paused", "fleet")
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID, "a draft is not paused");
    let a = s.set_mission_state(m.id, None, "active", "fleet").unwrap();
    let started = a.started_at.expect("started");
    let p = s.set_mission_state(m.id, None, "paused", "fleet").unwrap();
    let a = s.set_mission_state(m.id, None, "active", "fleet").unwrap();
    assert_eq!(
        a.started_at,
        Some(started),
        "a resume keeps the first start"
    );
    assert!(p.version < a.version);
    let done = s
        .set_mission_state(m.id, None, "completed", "fleet")
        .unwrap();
    assert!(done.finished_at.is_some());
    let e = s
        .set_mission_state(m.id, None, "active", "fleet")
        .unwrap_err();
    assert_eq!(
        e.code,
        codes::E_INVALID,
        "a finished mission stays finished"
    );
    let e = s
        .update_mission(m.id, None, &MissionPatch::default(), "fleet")
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID, "and does not change");
    assert_eq!(
        kinds(&s, m.id),
        ["created", "state", "state", "state", "state"]
    );
    // Reopen (G3.7): back to paused, never straight to active, and no
    // longer finished.
    let reopened = s.set_mission_state(m.id, None, "paused", "fleet").unwrap();
    assert_eq!(reopened.state, "paused");
    assert_eq!(reopened.finished_at, None);
    assert_eq!(
        reopened.started_at,
        Some(started),
        "a reopen keeps the start"
    );
    s.update_mission(m.id, None, &MissionPatch::default(), "fleet")
        .expect("a reopened mission changes again");
}

#[test]
fn every_finished_state_reopens_to_paused() {
    let s = store();
    for end in ["completed", "failed", "cancelled"] {
        let m = mission(&s, &format!("m-{end}"), None);
        s.set_mission_state(m.id, None, "active", "fleet").unwrap();
        s.set_mission_state(m.id, None, end, "fleet").unwrap();
        for refused in ["draft", "active"] {
            let e = s
                .set_mission_state(m.id, None, refused, "fleet")
                .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{end} → {refused}");
        }
        let r = s.set_mission_state(m.id, None, "paused", "fleet").unwrap();
        assert_eq!(r.state, "paused", "{end} reopens");
    }
}

#[test]
fn a_stale_version_writes_nothing() {
    let s = store();
    let m = mission(&s, "m", None);
    s.update_mission(
        m.id,
        Some(m.version),
        &MissionPatch {
            goal: Some("new goal".into()),
            done_when: Some(vec!["ci:test green".into(), " ".into()]),
            ..Default::default()
        },
        "fleet",
    )
    .unwrap();
    let e = s
        .update_mission(
            m.id,
            Some(m.version),
            &MissionPatch {
                name: Some("other".into()),
                ..Default::default()
            },
            "fleet",
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_CONFLICT);
    let now = s.get_mission(m.id).unwrap().unwrap();
    assert_eq!(now.name, "m");
    assert_eq!(now.goal, "new goal");
    assert_eq!(now.done_when, ["ci:test green"]);
}

#[test]
fn an_item_belongs_to_one_mission_and_the_root_stays() {
    let s = store();
    let root = local(&s, "epic");
    let a = mission(&s, "a", Some(root));
    let b = mission(&s, "b", None);
    let item = local(&s, "task");
    let m = s.set_mission_item(a.id, item, true, "fleet").unwrap();
    assert_eq!(m.total, 2);
    let e = s.set_mission_item(b.id, item, true, "fleet").unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    assert!(e.message.contains('a'), "{}", e.message);
    let e = s.set_mission_item(a.id, root, false, "fleet").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let m = s.set_mission_item(a.id, item, false, "fleet").unwrap();
    assert_eq!(m.total, 1);
    s.set_mission_item(b.id, item, true, "fleet").unwrap();
    assert_eq!(
        s.mission_items(a.id)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        [root]
    );
}

#[test]
fn a_mission_holds_at_most_the_cap_and_continuous_counts_open_work() {
    let s = store();
    let m = mission(&s, "m", None);
    for i in 0..MISSION_ITEM_CAP {
        let it = local(&s, &format!("t{i}"));
        s.set_mission_item(m.id, it, true, "fleet").unwrap();
    }
    let extra = local(&s, "one more");
    let e = s.set_mission_item(m.id, extra, true, "fleet").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    s.conn
        .execute(
            "UPDATE work_items SET status_category = 'done' WHERE orchestration_project_id = ?1",
            [m.id],
        )
        .unwrap();
    let e = s.set_mission_item(m.id, extra, true, "fleet").unwrap_err();
    assert_eq!(
        e.code,
        codes::E_INVALID,
        "a finite mission counts done work"
    );
    s.update_mission(
        m.id,
        None,
        &MissionPatch {
            mode: Some("continuous".into()),
            ..Default::default()
        },
        "fleet",
    )
    .unwrap();
    let m = s.set_mission_item(m.id, extra, true, "fleet").unwrap();
    assert_eq!(m.total, MISSION_ITEM_CAP + 1);
    assert_eq!(m.done, MISSION_ITEM_CAP);
}

#[test]
fn repos_are_an_allow_list_with_a_role() {
    let s = store();
    let m = mission(&s, "m", None);
    let p = s.upsert_project("acme", "api", "/src/api").unwrap();
    let r = s
        .set_mission_repo(m.id, p, Some("backend"), true, "fleet")
        .unwrap();
    assert_eq!(r.repos.len(), 1);
    assert_eq!(r.repos[0].name, "acme/api");
    assert_eq!(r.repos[0].role.as_deref(), Some("backend"));
    let r = s.set_mission_repo(m.id, p, None, true, "fleet").unwrap();
    assert_eq!(r.repos[0].role, None, "a second add changes the role");
    let e = s
        .set_mission_repo(m.id, 9999, None, true, "fleet")
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let r = s.set_mission_repo(m.id, p, None, false, "fleet").unwrap();
    assert!(r.repos.is_empty());
    assert_eq!(
        kinds(&s, m.id),
        ["created", "repo_added", "repo_added", "repo_removed"]
    );
}

#[test]
fn a_running_mission_is_not_deleted_and_its_items_survive() {
    let s = store();
    let root = local(&s, "epic");
    let m = mission(&s, "m", Some(root));
    s.set_mission_state(m.id, None, "active", "fleet").unwrap();
    assert_eq!(s.delete_mission(m.id).unwrap_err().code, codes::E_INVALID);
    s.set_mission_state(m.id, None, "cancelled", "fleet")
        .unwrap();
    assert!(s.delete_mission(m.id).unwrap());
    assert!(s.get_work_item(root).unwrap().is_some());
    assert_eq!(s.item_mission(root).unwrap(), None);
    assert!(!s.delete_mission(m.id).unwrap());
}

#[test]
fn a_decision_is_logged_once() {
    let s = store();
    let m = mission(&s, "m", None);
    let e = NewMissionEvent {
        kind: "planned",
        actor: "planner:1",
        decision_id: Some("d-1"),
        ..Default::default()
    };
    assert!(s.record_mission_event(m.id, &e).unwrap().is_some());
    assert!(s.record_mission_event(m.id, &e).unwrap().is_none());
    assert_eq!(kinds(&s, m.id), ["created", "planned"]);
}

#[test]
fn the_log_folds_its_oldest_rows_into_one_digest() {
    let s = store();
    let m = mission(&s, "m", None);
    for _ in 0..MISSION_EVENT_CAP + 2 {
        s.record_mission_event(
            m.id,
            &NewMissionEvent {
                kind: "task_done",
                actor: "fleet",
                ..Default::default()
            },
        )
        .unwrap();
    }
    let n: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM orchestration_events WHERE orchestration_project_id = ?1",
            [m.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, MISSION_EVENT_CAP + 1, "the cap plus the digest");
    let digest: String = s
        .conn
        .query_row(
            "SELECT payload FROM orchestration_events WHERE kind = 'digest'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&digest).unwrap();
    assert_eq!(v["kinds"]["created"], 1);
    assert_eq!(v["kinds"]["task_done"], 2);
}
