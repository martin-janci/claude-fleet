//! The deterministic loop's steps (orchestration O4).

use super::*;
use crate::service::work::verify::{CondCheck, Verification};
use crate::store::{MissionPolicy, TaskReport};

fn mission(policy: MissionPolicy) -> MissionRow {
    MissionRow {
        id: 1,
        org_id: None,
        owner_person_id: None,
        root_item_id: Some(10),
        name: "m".into(),
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
        total: 0,
        done: 0,
        repos: vec![],
        policy,
        next_wake_at: None,
        cost_micros: None,
        budget_micros: None,
        waiting_on: None,
    }
}

fn item(id: i64, status: &str) -> WorkItemRow {
    let mut i: WorkItemRow = serde_json::from_value(serde_json::json!({
        "id": id, "source": "local", "title": format!("t{id}"),
        "status_category": status, "created_at": 0, "updated_at": 0
    }))
    .unwrap();
    i.key = Some(format!("TASK-{id}"));
    i
}

fn node(id: i64, state: &str) -> GraphNode {
    GraphNode {
        item_id: id,
        state: state.into(),
        wave: 1,
        depends_on: vec![],
        waiting_for: vec![],
        verification: None,
        attempt: None,
    }
}

fn task(id: i64, role: &str, state: &str, error: Option<&str>) -> TaskRow {
    let mut t: TaskRow = serde_json::from_value(serde_json::json!({
        "id": id, "state": state, "created_at": 0, "role": role
    }))
    .unwrap();
    t.error = error.map(str::to_string);
    t
}

fn plan(
    m: &MissionRow,
    nodes: Vec<GraphNode>,
    items: &[WorkItemRow],
    attempts: HashMap<i64, Vec<TaskRow>>,
    open: i64,
) -> Vec<Step> {
    let graph = MissionGraph {
        nodes,
        waves: 1,
        outside: vec![],
    };
    plan_steps(&StepInput {
        mission: m,
        graph: &graph,
        items,
        attempts: &attempts,
        counts: MissionTaskCounts {
            total: attempts.values().map(|v| v.len() as i64).sum(),
            open,
            last_activity_at: None,
        },
        max_parallel: m.policy.max_parallel,
    })
}

fn kinds(steps: &[Step]) -> Vec<String> {
    steps.iter().map(Step::key).collect()
}

#[test]
fn ready_items_run_up_to_the_parallelism_and_the_root_waits() {
    let m = mission(MissionPolicy::default());
    let items = [
        item(10, "todo"),
        item(11, "todo"),
        item(12, "todo"),
        item(13, "todo"),
    ];
    let nodes = vec![
        node(10, "ready"),
        node(11, "ready"),
        node(12, "ready"),
        node(13, "ready"),
    ];
    let s = plan(&m, nodes.clone(), &items, HashMap::new(), 0);
    assert_eq!(
        kinds(&s),
        vec!["run:11", "run:12"],
        "two slots; the root is the container"
    );
    let s = plan(&m, nodes, &items, HashMap::new(), 2);
    assert!(s.is_empty(), "both slots are taken");
    // A root alone is the whole mission, and runs.
    let s = plan(
        &m,
        vec![node(10, "ready")],
        &[item(10, "todo")],
        HashMap::new(),
        0,
    );
    assert_eq!(kinds(&s), vec!["run:10"]);
}

#[test]
fn a_failure_retries_with_its_error_then_asks() {
    let m = mission(MissionPolicy::default());
    let items = [item(10, "todo"), item(11, "todo")];
    let nodes = vec![node(10, "waiting"), node(11, "failed")];
    let once = HashMap::from([(11, vec![task(1, "implement", "failed", Some("tests red"))])]);
    let s = plan(&m, nodes.clone(), &items, once, 0);
    assert_eq!(kinds(&s), vec!["retry:11"]);
    assert_eq!(s[0].context.as_deref(), Some("tests red"));
    let twice_same = HashMap::from([(
        11,
        vec![
            task(2, "implement", "failed", Some("tests red")),
            task(1, "implement", "failed", Some("tests red")),
        ],
    )]);
    let s = plan(&m, nodes.clone(), &items, twice_same, 0);
    assert_eq!(kinds(&s), vec!["ask:11"]);
    assert!(!s[0].auto, "only a person answers");
    let twice_new = HashMap::from([(
        11,
        vec![
            task(2, "implement", "failed", Some("lint")),
            task(1, "implement", "failed", Some("tests red")),
        ],
    )]);
    let s = plan(&m, nodes, &items, twice_new, 0);
    assert_eq!(
        kinds(&s),
        vec!["ask:11"],
        "one retry, then the retries are spent"
    );
}

#[test]
fn finished_work_goes_to_review_and_test_and_closes_once_verified() {
    let m = mission(MissionPolicy::default());
    let items = [item(10, "todo"), item(11, "todo")];
    let check = |line: &str, kind: &str, state: &str| CondCheck {
        line: line.into(),
        kind: kind.into(),
        state: state.into(),
        detail: String::new(),
        by: None,
        at: None,
    };
    let mut n = node(11, "verifying");
    n.verification = Some(Verification {
        state: "unverified".into(),
        checks: vec![
            check("review", "review", "pending"),
            check("test:cargo test", "test", "pending"),
            check("person", "person", "pending"),
        ],
    });
    let done = HashMap::from([(11, vec![task(1, "implement", "done", None)])]);
    let s = plan(
        &m,
        vec![node(10, "waiting"), n.clone()],
        &items,
        done.clone(),
        0,
    );
    assert_eq!(kinds(&s), vec!["review:11", "test:11"]);
    assert_eq!(s[1].context.as_deref(), Some("Run exactly: cargo test"));
    n.verification = Some(Verification {
        state: "failed".into(),
        checks: vec![CondCheck {
            detail: "the reviewer answered partial: tests missing".into(),
            ..check("review", "review", "fail")
        }],
    });
    let s = plan(
        &m,
        vec![node(10, "waiting"), n.clone()],
        &items,
        done.clone(),
        0,
    );
    assert_eq!(kinds(&s), vec!["retry:11"]);
    assert!(s[0].context.as_deref().unwrap().contains("tests missing"));
    n.verification = Some(Verification {
        state: "verified".into(),
        checks: vec![],
    });
    let s = plan(&m, vec![node(10, "waiting"), n], &items, done, 0);
    assert_eq!(kinds(&s), vec!["close:11"]);
}

#[test]
fn review_by_policy_and_completion_when_all_is_done() {
    let m = mission(MissionPolicy {
        require_review: true,
        ..Default::default()
    });
    let items = [item(10, "todo"), item(11, "todo")];
    let done = HashMap::from([(11, vec![task(1, "implement", "done", None)])]);
    let s = plan(
        &m,
        vec![node(10, "waiting"), node(11, "verifying")],
        &items,
        done,
        0,
    );
    assert_eq!(kinds(&s), vec!["review:11"]);
    let items = [item(10, "todo"), item(11, "done")];
    let s = plan(
        &m,
        vec![node(10, "ready"), node(11, "done")],
        &items,
        HashMap::new(),
        0,
    );
    assert_eq!(kinds(&s), vec!["complete:10"]);
    assert!(
        !s[0].auto,
        "a person completes a mission, never the loop (review round 15, F19)"
    );
    let cont = MissionRow {
        mode: "continuous".into(),
        ..m
    };
    assert!(plan(
        &cont,
        vec![node(10, "ready"), node(11, "done")],
        &items,
        HashMap::new(),
        0
    )
    .is_empty());
}

#[test]
fn the_task_budget_stops_new_runs_with_one_ask() {
    let m = mission(MissionPolicy {
        max_tasks: 1,
        ..Default::default()
    });
    let items = [
        item(10, "todo"),
        item(11, "todo"),
        item(12, "todo"),
        item(13, "done"),
    ];
    let used = HashMap::from([(13, vec![task(1, "implement", "done", None)])]);
    let s = plan(
        &m,
        vec![
            node(10, "waiting"),
            node(11, "ready"),
            node(12, "ready"),
            node(13, "done"),
        ],
        &items,
        used,
        0,
    );
    assert_eq!(kinds(&s), vec!["ask:0"]);
}

#[test]
fn a_reported_failure_counts_as_one() {
    let mut t = task(1, "implement", "done", None);
    t.report = Some(TaskReport {
        outcome: "blocked".into(),
        ..Default::default()
    });
    assert!(attempt_failed(&t));
    t.report.as_mut().unwrap().outcome = "done".into();
    assert!(!attempt_failed(&t));
}

fn reviewed(id: i64, outcome: &str) -> TaskRow {
    let mut t = task(id, "review", "done", None);
    t.report = Some(TaskReport {
        outcome: outcome.into(),
        summary: format!("review {outcome}"),
        ..Default::default()
    });
    t
}

#[test]
fn a_finished_review_settles_an_item_without_lines() {
    let m = mission(MissionPolicy {
        require_review: true,
        ..Default::default()
    });
    let items = [item(10, "todo"), item(11, "todo")];
    let nodes = || vec![node(10, "waiting"), node(11, "verifying")];
    // Approved: the item closes instead of asking for another review.
    let approved = HashMap::from([(
        11,
        vec![reviewed(2, "done"), task(1, "implement", "done", None)],
    )]);
    assert_eq!(
        kinds(&plan(&m, nodes(), &items, approved, 0)),
        vec!["close:11"]
    );
    // Changes asked for: the work runs again.
    let rejected = HashMap::from([(
        11,
        vec![
            reviewed(2, "changes_requested"),
            task(1, "implement", "done", None),
        ],
    )]);
    assert_eq!(
        kinds(&plan(&m, nodes(), &items, rejected, 0)),
        vec!["retry:11"]
    );
    // The retry finished: the old review does not count for it.
    let redone = HashMap::from([(
        11,
        vec![
            task(3, "implement", "done", None),
            reviewed(2, "changes_requested"),
            task(1, "implement", "done", None),
        ],
    )]);
    assert_eq!(
        kinds(&plan(&m, nodes(), &items, redone, 0)),
        vec!["review:11"]
    );
}
