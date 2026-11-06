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

/// A graph of `states` in graph order; a verifying item's test is due.
fn graph(states: &[(i64, &str)]) -> crate::service::work::graph::MissionGraph {
    let nodes: Vec<serde_json::Value> = states
        .iter()
        .map(|(id, st)| {
            let mut n = serde_json::json!({"item_id": id, "state": st, "wave": 1});
            if *st == "verifying" {
                n["verification"] = serde_json::json!({"state": "unverified", "checks": [
                    {"line": "test:cargo test", "kind": "test", "state": "pending", "detail": ""}]});
            }
            n
        })
        .collect();
    serde_json::from_value(serde_json::json!({"nodes": nodes, "waves": 1})).unwrap()
}

/// Mission improvements M5: with a lane's tests at its end, the lane runs
/// one test, on its last task whose test is due, once none of its tasks
/// still moves and it is not busy; a task in no lane tests as before.
#[test]
fn a_lanes_one_test_waits_for_its_last_task() {
    let none = HashSet::new();
    let all_in = graph(&[(10, "verifying"), (11, "verifying"), (30, "verifying")]);
    assert_eq!(
        held_lane_tests(&all_in, &lane_of(), &none),
        HashSet::from([10])
    );
    let still_running = graph(&[(10, "verifying"), (11, "running"), (30, "verifying")]);
    assert_eq!(
        held_lane_tests(&still_running, &lane_of(), &none),
        HashSet::from([10]),
        "lane 1 still implements"
    );
    assert_eq!(
        held_lane_tests(&all_in, &lane_of(), &HashSet::from([1])),
        HashSet::from([10, 11]),
        "a busy lane tests nothing"
    );
}

/// M5: a lane whose second task depends on its first tests the first
/// while the second waits for it to be done; a ready task holds the test.
#[test]
fn a_lane_whose_tasks_depend_on_each_other_still_tests() {
    let none = HashSet::new();
    for left in ["waiting", "blocked", "held"] {
        let g = graph(&[(10, "verifying"), (11, left)]);
        assert!(held_lane_tests(&g, &lane_of(), &none).is_empty(), "{left}");
    }
    let g = graph(&[(10, "verifying"), (11, "ready")]);
    assert_eq!(held_lane_tests(&g, &lane_of(), &none), HashSet::from([10]));
}

/// M5: a held lane test takes no slot, so ready work elsewhere still runs.
#[test]
fn a_held_lane_test_leaves_its_slot_to_ready_work() {
    use crate::service::work::orchestrate::steps::{plan_steps_holding, StepInput};
    let mut m = mission_row();
    m.policy.max_parallel = 2;
    let g = graph(&[(10, "verifying"), (11, "verifying"), (30, "ready")]);
    let items: Vec<crate::store::WorkItemRow> = [10, 11, 30]
        .iter()
        .map(|id| {
            serde_json::from_value(serde_json::json!({
                "id": id, "source": "local", "title": format!("t{id}"),
                "status_category": "todo", "created_at": 0, "updated_at": 0
            }))
            .unwrap()
        })
        .collect();
    let held = held_lane_tests(&g, &lane_of(), &HashSet::new());
    let steps = plan_steps_holding(
        &StepInput {
            mission: &m,
            graph: &g,
            items: &items,
            attempts: &HashMap::new(),
            counts: crate::store::MissionTaskCounts {
                total: 0,
                open: 0,
                last_activity_at: None,
            },
            max_parallel: 2,
        },
        &held,
    );
    assert_eq!(keys(&steps), ["test:11", "run:30"]);
}

/// Mission improvements M4: under `lane_pr = draft` the brief tells the
/// lead that fleet pushes and opens the PR; otherwise it asks the lead to.
#[test]
fn the_brief_says_who_opens_the_lanes_pull_request() {
    let mut m = mission_row();
    let own = lane_brief(&m, "api", &[]);
    assert!(
        own.contains("open a draft pull request for this branch"),
        "{own}"
    );
    m.policy.lane_pr = "draft".into();
    let fleet = lane_brief(&m, "api", &[]);
    assert!(fleet.contains("Fleet pushes this branch"), "{fleet}");
    assert!(fleet.contains("never mark it ready or merge it"));
}
