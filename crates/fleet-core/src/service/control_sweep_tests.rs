use super::*;
use crate::service::outcome::{CheckSummary, PrEvidence};
use crate::store::{MissionPolicy, SessionOrigin};

fn store_with_row() -> (Store, SessionRow) {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let id = s
        .upsert_session("fed-v2", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let r = s.get_session_by_id(id).unwrap().unwrap();
    (s, r)
}

fn with_pr(mut r: SessionRow, merge_state: &str, failing: u32, head: &str) -> SessionRow {
    r.pr_url = Some("https://github.com/o/r/pull/7".into());
    r.pr_evidence = Some(PrEvidence {
        head_oid: Some(head.into()),
        merge_state: Some(merge_state.into()),
        state: Some("OPEN".into()),
        checks: CheckSummary {
            failing_total: failing,
            ..Default::default()
        },
        ..Default::default()
    });
    r
}

fn mission(done: i64, total: i64) -> MissionRow {
    MissionRow {
        id: 4,
        org_id: None,
        owner_person_id: None,
        root_item_id: None,
        name: "ship it".into(),
        goal: "g".into(),
        non_goals: None,
        done_when: vec![],
        mode: "finite".into(),
        state: "active".into(),
        level: 2,
        plan_version: 1,
        created_at: 0,
        updated_at: 0,
        started_at: None,
        finished_at: None,
        version: 1,
        total,
        done,
        repos: vec![],
        policy: MissionPolicy::default(),
        next_wake_at: None,
        cost_micros: None,
        budget_micros: None,
        waiting_on: None,
    }
}

#[test]
fn a_conflicting_or_red_pr_is_a_sweep_line_keyed_by_its_head() {
    let (_s, r) = store_with_row();
    let c = pr_condition(&with_pr(r.clone(), "DIRTY", 0, "abc"), false).unwrap();
    assert!(c.text.starts_with(ACT), "{}", c.text);
    assert!(c.text.contains("merge conflicts") && c.text.contains("fed-v2"));
    assert_eq!(c.key, format!("sw:pr:{}:abc:conflict", r.id));

    let red = pr_condition(&with_pr(r.clone(), "CLEAN", 2, "abc"), true).unwrap();
    assert!(red.text.starts_with(REPORT), "{}", red.text);
    assert!(red.text.contains("failing checks"));
    // A new push that is still red is a new change.
    let pushed = pr_condition(&with_pr(r.clone(), "CLEAN", 1, "def"), true).unwrap();
    assert_ne!(red.key, pushed.key);
}

#[test]
fn a_clean_behind_or_missing_pr_is_no_line() {
    let (_s, r) = store_with_row();
    assert!(pr_condition(&r, false).is_none());
    assert!(pr_condition(&with_pr(r.clone(), "CLEAN", 0, "a"), false).is_none());
    assert!(pr_condition(&with_pr(r.clone(), "BEHIND", 0, "a"), false).is_none());
    let mut merged = with_pr(r, "DIRTY", 3, "a");
    merged.pr_evidence.as_mut().unwrap().state = Some("MERGED".into());
    assert!(pr_condition(&merged, false).is_none());
}

#[test]
fn a_mission_stalls_once_its_done_count_stands_still_long_enough() {
    let mut st = SweepState::default();
    let m = mission(1, 5);
    let t0 = 1_000_000;
    assert_eq!(progress_since(&mut st, &m, t0), t0);
    assert_eq!(progress_since(&mut st, &m, t0 + 100), t0);
    assert!(mission_condition(&m, t0, t0 + STALL_SECS - 1, false).is_none());
    let c = mission_condition(&m, t0, t0 + STALL_SECS, false).unwrap();
    assert!(
        c.text.starts_with(ACT) && c.text.contains("mission #4"),
        "{}",
        c.text
    );
    // Progress restarts the clock and is a new key when it stalls again.
    let moved = mission(2, 5);
    assert_eq!(progress_since(&mut st, &moved, t0 + 9000), t0 + 9000);
    let again = mission_condition(&moved, t0 + 9000, t0 + 9000 + STALL_SECS, true).unwrap();
    assert_ne!(again.key, c.key);
    assert!(again.text.starts_with(REPORT));
}

#[test]
fn a_waiting_finished_or_paused_mission_never_stalls() {
    let t = STALL_SECS * 10;
    let mut waiting = mission(0, 3);
    waiting.waiting_on = Some(
        serde_json::from_value(
            serde_json::json!({"reason": "question", "since": 0, "open_cards": 0}),
        )
        .unwrap(),
    );
    assert!(mission_condition(&waiting, 0, t, false).is_none());
    assert!(mission_condition(&mission(3, 3), 0, t, false).is_none());
    let mut paused = mission(0, 3);
    paused.state = "paused".into();
    assert!(mission_condition(&paused, 0, t, false).is_none());
}

#[test]
fn the_sweep_looks_at_what_control_started_and_nothing_with_the_setting_off() {
    let (s, r) = store_with_row();
    let other = s
        .upsert_session("other", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_session_origin(
        r.id,
        &SessionOrigin {
            origin: "operator",
            origin_ref: None,
        },
    )
    .unwrap();
    assert_eq!(s.handed_off_session_ids(None).unwrap(), vec![r.id]);
    assert!(s.handed_off_session_ids(Some(r.id)).unwrap().is_empty());
    assert!(!s.handed_off_session_ids(None).unwrap().contains(&other));
    s.set_setting(settings::CONTROL_SWEEP, "false").unwrap();
    assert!(conditions(&s, None, 0).is_empty());
}

#[test]
fn only_a_thread_control_still_follows_is_swept() {
    use crate::service::control_follow::{Follow, FollowState};
    let f = |kind: &str, id: i64, state: FollowState| Follow {
        kind: kind.into(),
        id,
        state,
        name: String::new(),
        host_alias: None,
        muted: state == FollowState::Muted,
    };
    let follows = [
        f("session", 1, FollowState::Following),
        f("session", 2, FollowState::Muted),
        f("session", 3, FollowState::Quiet),
        f("session", 4, FollowState::Merged),
        f("mission", 1, FollowState::Following),
        f("mission", 5, FollowState::Muted),
    ];
    assert!(swept(&follows, "session", 1));
    assert!(!swept(&follows, "session", 2));
    assert!(!swept(&follows, "session", 3));
    assert!(!swept(&follows, "session", 4));
    assert!(!swept(&follows, "session", 9), "not a thread of Control's");
    assert!(swept(&follows, "mission", 1));
    assert!(!swept(&follows, "mission", 5));
}
