//! A mission loop's lease, wakes, cards and grants (orchestration O4–O6).

use super::*;
use crate::store::NewMission;

fn mission(s: &Store) -> i64 {
    s.create_mission(
        &NewMission {
            name: "m",
            goal: "g",
            ..Default::default()
        },
        "fleet",
    )
    .unwrap()
    .id
}

#[test]
fn only_an_active_mission_wakes_and_one_tick_holds_it() {
    let s = Store::open_in_memory().unwrap();
    let m = mission(&s);
    let now = now_unix();
    assert!(
        s.missions_due(now).unwrap().is_empty(),
        "a draft is not due"
    );
    s.set_mission_state(m, None, "active", "fleet").unwrap();
    assert_eq!(s.missions_due(now).unwrap(), vec![m], "no wake yet is due");
    assert!(s.take_mission_lease(m, now).unwrap());
    assert!(!s.take_mission_lease(m, now).unwrap(), "held");
    assert!(s.missions_due(now).unwrap().is_empty());
    s.release_mission_lease(m, Some(now + 600), Some(now))
        .unwrap();
    assert!(
        s.missions_due(now).unwrap().is_empty(),
        "not due before its wake"
    );
    s.wake_mission(m).unwrap();
    assert_eq!(s.missions_due(now_unix()).unwrap(), vec![m]);
    // A lease left by a crashed tick expires.
    assert!(s.take_mission_lease(m, now).unwrap());
    assert!(s
        .take_mission_lease(m, now + MISSION_LEASE_SECS + 1)
        .unwrap());
}

#[test]
fn a_repeated_decision_at_the_cap_is_still_one_card_not_an_error() {
    let s = Store::open_in_memory().unwrap();
    let m = mission(&s);
    let card = |d: String| NewCard {
        decision_id: Box::leak(d.into_boxed_str()),
        source: "loop",
        kind: "ask",
        ..Default::default()
    };
    for i in 0..CARDS_OPEN_CAP {
        s.add_card(m, &card(format!("d{i}"))).unwrap().unwrap();
    }
    assert_eq!(s.add_card(m, &card("d0".into())).unwrap(), None);
    assert_eq!(
        s.add_card(m, &card("new".into())).unwrap_err().code,
        codes::E_LIMIT
    );
}

#[test]
fn a_decision_is_one_card_and_a_card_closes_once() {
    let s = Store::open_in_memory().unwrap();
    let m = mission(&s);
    let c = NewCard {
        decision_id: "d1",
        source: "planner",
        kind: "ask",
        payload: Some(serde_json::json!({ "question": "which db?" })),
        ..Default::default()
    };
    let card = s.add_card(m, &c).unwrap().unwrap();
    assert_eq!(s.add_card(m, &c).unwrap(), None, "the same decision again");
    assert_eq!(
        s.add_card(
            m,
            &NewCard {
                kind: "deploy",
                decision_id: "d2",
                ..c.clone()
            }
        )
        .unwrap_err()
        .code,
        codes::E_INVALID
    );
    assert!(s
        .decide_card(card.id, "dismissed", "person:1", None)
        .unwrap());
    assert!(!s.decide_card(card.id, "applied", "person:1", None).unwrap());
    let cards = s.mission_cards(m, 10).unwrap();
    assert_eq!(cards[0].state, "dismissed");
    assert_eq!(cards[0].payload.as_ref().unwrap()["question"], "which db?");
}

#[test]
fn a_grant_holds_until_it_expires_is_revoked_or_the_plan_changes() {
    let s = Store::open_in_memory().unwrap();
    let m = mission(&s);
    let now = now_unix();
    assert_eq!(s.live_mission_grant(m, now).unwrap(), None);
    let g = s
        .add_grant(
            m,
            &NewGrant {
                level: 2,
                granted_by: "person:1",
                hosts: Some(vec!["h".into()]),
                budget_micros: Some(5_000_000),
                expires_at: now + 3600,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.live_mission_grant(m, now).unwrap().unwrap().id, g.id);
    assert_eq!(
        s.live_mission_grant(m, now + 3601).unwrap(),
        None,
        "expired"
    );
    s.bump_plan_version(m).unwrap();
    assert_eq!(
        s.live_mission_grant(m, now).unwrap(),
        None,
        "signed for the old plan"
    );
    let g2 = s
        .add_grant(
            m,
            &NewGrant {
                level: 1,
                granted_by: "person:1",
                expires_at: now + 60,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.live_mission_grant(m, now).unwrap().unwrap().id, g2.id);
    assert_eq!(
        s.revoke_grants(m).unwrap(),
        2,
        "the void one is revoked too"
    );
    assert_eq!(s.live_mission_grant(m, now).unwrap(), None);
}

#[test]
fn counts_and_cost_cover_the_members_attempts() {
    let s = Store::open_in_memory().unwrap();
    let m = mission(&s);
    s.upsert_host("h").unwrap();
    let w = s
        .upsert_session("w", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let item = s.create_local_work_item(None, "a").unwrap().id;
    s.set_mission_item(m, item, true, "fleet").unwrap();
    let t = s.insert_task(None, Some(w), "go", "n").unwrap();
    s.set_task_run(t.id, item, 1, "implement").unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sessions SET usage_cost_micros = 1500000 WHERE id = ?1",
            [w],
        )
        .unwrap();
    let c = s.mission_task_counts(m).unwrap();
    assert_eq!((c.total, c.open), (1, 1));
    assert_eq!(s.mission_cost_micros(m).unwrap(), 1_500_000);
}

/// An active mission, its next wake an hour away.
fn sleeping(s: &Store) -> i64 {
    let m = mission(s);
    s.set_mission_state(m, None, "active", "fleet").unwrap();
    let now = now_unix();
    assert!(s.take_mission_lease(m, now).unwrap());
    s.release_mission_lease(m, Some(now + 3600), Some(now))
        .unwrap();
    assert!(s.missions_due(now).unwrap().is_empty());
    m
}

#[test]
fn resuming_a_paused_mission_wakes_it() {
    let s = Store::open_in_memory().unwrap();
    let m = sleeping(&s);
    s.set_mission_state(m, None, "paused", "fleet").unwrap();
    s.set_mission_state(m, None, "active", "fleet").unwrap();
    assert_eq!(
        s.missions_due(now_unix()).unwrap(),
        vec![m],
        "a resumed mission does not sleep out the wake its brake set"
    );
}

#[test]
fn a_members_session_wakes_its_mission_and_another_does_not() {
    let s = Store::open_in_memory().unwrap();
    let m = sleeping(&s);
    s.upsert_host("h").unwrap();
    let w = s
        .upsert_session("w", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let other = s
        .upsert_session("o", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let item = s.create_local_work_item(None, "a").unwrap().id;
    s.set_mission_item(m, item, true, "fleet").unwrap();
    let t = s.insert_task(None, Some(w), "go", "n").unwrap();
    s.set_task_run(t.id, item, 1, "implement").unwrap();

    assert_eq!(s.wake_session_missions(other).unwrap(), 0);
    assert!(s.missions_due(now_unix()).unwrap().is_empty());
    assert_eq!(s.wake_session_missions(w).unwrap(), 1);
    assert_eq!(s.missions_due(now_unix()).unwrap(), vec![m]);
}

#[test]
fn a_members_status_moving_on_the_tracker_wakes_its_mission() {
    let s = Store::open_in_memory().unwrap();
    let m = sleeping(&s);
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap()
        .id;
    let write = |status: (&str, &str)| crate::store::TrackerItemWrite {
        external_id: "1".into(),
        key: Some("ABC-1".into()),
        title: "a".into(),
        status_name: status.0.into(),
        status_category: status.1.into(),
        ..Default::default()
    };
    let item = s
        .upsert_tracker_item(t, &write(("To Do", "todo")))
        .unwrap()
        .id;
    s.set_mission_item(m, item, true, "fleet").unwrap();
    // A re-read that changes nothing leaves it asleep.
    s.upsert_tracker_item(t, &write(("To Do", "todo"))).unwrap();
    assert!(s.missions_due(now_unix()).unwrap().is_empty());
    s.upsert_tracker_item(t, &write(("Done", "done"))).unwrap();
    assert_eq!(s.missions_due(now_unix()).unwrap(), vec![m]);
}
