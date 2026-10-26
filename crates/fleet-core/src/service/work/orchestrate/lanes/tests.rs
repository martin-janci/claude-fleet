//! Lane execution's pure rules (lanes design L3) and its busy reading.

use super::*;

fn step(kind: &str, item: i64, role: &str) -> Step {
    Step {
        kind: kind.into(),
        item_id: Some(item),
        role: Some(role.into()),
        reason: String::new(),
        context: None,
        auto: true,
    }
}

fn keys(steps: &[Step]) -> Vec<String> {
    steps.iter().map(Step::key).collect()
}

/// Lane 1 holds items 10 and 11, lane 2 holds 20; 30 is in no lane.
fn lane_of() -> HashMap<i64, i64> {
    HashMap::from([(10, 1), (11, 1), (20, 2)])
}

#[test]
fn a_lane_takes_its_first_implement_run_and_others_pass() {
    let steps = vec![
        step("run", 10, "implement"),
        step("run", 11, "implement"),
        step("run", 20, "implement"),
        step("run", 30, "implement"),
        step("review", 11, "review"),
    ];
    let got = gate(steps, &lane_of(), &HashSet::new());
    assert_eq!(keys(&got), ["run:10", "run:20", "run:30", "review:11"]);
}

#[test]
fn a_busy_lane_takes_none_and_a_retry_counts_as_its_run() {
    let steps = vec![
        step("retry", 11, "implement"),
        step("run", 10, "implement"),
        step("run", 20, "implement"),
    ];
    let got = gate(steps.clone(), &lane_of(), &HashSet::from([2]));
    assert_eq!(
        keys(&got),
        ["retry:11"],
        "lane 2 is busy; lane 1 takes the retry"
    );
}

#[test]
fn a_lead_hands_over_after_its_share_or_a_second_failure_here() {
    assert!(!needs_relay(3, 0, 4, None));
    assert!(needs_relay(4, 0, 4, None), "its share of tasks");
    assert!(
        !needs_relay(1, 1, 4, Some(40.0)),
        "the first retry stays in its context"
    );
    assert!(needs_relay(1, 2, 4, None), "the next gets fresh eyes");
    assert!(
        needs_relay(1, 0, 4, Some(72.0)),
        "a context that is filling up"
    );
}

fn mission_row() -> MissionRow {
    serde_json::from_value(serde_json::json!({
        "id": 1, "name": "Payments", "goal": "Cards and refunds", "non_goals": "No UI",
        "mode": "finite", "state": "active", "level": 2, "plan_version": 1,
        "created_at": 0, "updated_at": 0, "version": 1
    }))
    .unwrap()
}

#[test]
fn the_brief_names_the_goal_the_tasks_in_order_and_the_rules() {
    let b = lane_brief(
        &mission_row(),
        "backend",
        &["T-1 Schema".to_string(), "T-2 API".to_string()],
    );
    assert!(b.starts_with("You are the lead of lane \"backend\" of mission \"Payments\"."));
    assert!(b.contains("Goal: Cards and refunds") && b.contains("Not in scope: No UI"));
    assert!(b.contains("1. T-1 Schema\n2. T-2 API\n"), "{b}");
    assert!(b.contains("never amend, rebase or squash"));
    assert!(b.contains("never mark it ready or merge it"));
    assert!(b.chars().count() <= crate::service::work::handover::BRIEF_MAX_CHARS);
}

#[test]
fn a_task_prompt_carries_the_task_a_retrys_words_and_the_subagent_note() {
    let p = task_prompt(
        "T-2",
        "API",
        Some("REST only"),
        "implement",
        Some("tests\nfail"),
        true,
    );
    assert!(
        p.starts_with("Next task in your lane.\n\nWork on T-2 API.\n\nREST only"),
        "{p}"
    );
    assert!(p.contains("> tests\n> fail"), "{p}");
    assert!(p.contains("Agent tool"));
    let bare = task_prompt("T-2", "API", None, "implement", None, false);
    assert!(!bare.contains("Agent tool") && !bare.contains("earlier attempts"));
}

/// A lane is busy while a task of it has an open implement run; a review
/// beside it does not hold it, and a lead that is no longer the mission's
/// running session is no lead.
#[test]
fn a_lane_is_busy_while_one_of_its_tasks_runs() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let m = mission_row();
    let lane: MissionLaneRow = serde_json::from_value(serde_json::json!({
        "id": 1, "mission_id": 1, "name": "backend", "state": "idle",
        "created_at": 0, "updated_at": 0, "item_ids": [10, 11]
    }))
    .unwrap();
    let task = |id: i64, item: i64, role: &str, state: &str| -> TaskRow {
        serde_json::from_value(serde_json::json!({
            "id": id, "work_item_id": item, "role": role, "state": state, "created_at": 0
        }))
        .unwrap()
    };
    let mut attempts = HashMap::from([(10, vec![task(1, 10, "implement", "done")])]);
    assert!(busy_lanes(
        &s,
        &m,
        std::slice::from_ref(&lane),
        &attempts,
        &HashSet::new()
    )
    .unwrap()
    .is_empty());
    attempts.insert(11, vec![task(2, 11, "review", "running")]);
    assert!(busy_lanes(
        &s,
        &m,
        std::slice::from_ref(&lane),
        &attempts,
        &HashSet::new()
    )
    .unwrap()
    .is_empty());
    attempts.insert(11, vec![task(3, 11, "implement", "running")]);
    assert_eq!(
        busy_lanes(
            &s,
            &m,
            std::slice::from_ref(&lane),
            &attempts,
            &HashSet::new()
        )
        .unwrap(),
        HashSet::from([1])
    );
    // A lead id whose row is someone else's session is no lead.
    let other = s
        .upsert_session("someone", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let led = MissionLaneRow {
        lead_session_id: Some(other),
        ..lane
    };
    assert_eq!(live_lead(&s, &m, &led).unwrap(), None);
}

/// Lanes design L4 (§3.7): a lane waits while a task of it waits for its
/// test or runs one; a review beside it does not hold it.
#[test]
fn a_lane_waits_for_its_tasks_test() {
    let s = Store::open_in_memory().unwrap();
    let m = mission_row();
    let lane: MissionLaneRow = serde_json::from_value(serde_json::json!({
        "id": 1, "mission_id": 1, "name": "backend", "state": "idle",
        "created_at": 0, "updated_at": 0, "item_ids": [10]
    }))
    .unwrap();
    let lanes = std::slice::from_ref(&lane);
    let none = HashMap::new();
    assert_eq!(
        busy_lanes(&s, &m, lanes, &none, &HashSet::from([10])).unwrap(),
        HashSet::from([1])
    );
    let graph: crate::service::work::graph::MissionGraph =
        serde_json::from_value(serde_json::json!({
            "nodes": [
                {"item_id": 10, "state": "verifying", "wave": 1,
                 "verification": {"state": "pending", "checks": [
                    {"line": "test:cargo test", "kind": "test", "state": "pending", "detail": ""},
                    {"line": "review", "kind": "review", "state": "pass", "detail": ""}]}},
                {"item_id": 11, "state": "verifying", "wave": 1,
                 "verification": {"state": "pending", "checks": [
                    {"line": "review", "kind": "review", "state": "pending", "detail": ""}]}},
                {"item_id": 12, "state": "ready", "wave": 1}
            ],
            "waves": 1
        }))
        .unwrap();
    assert_eq!(awaiting_test(&graph), HashSet::from([10]));
}
