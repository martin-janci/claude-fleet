//! K3 `mission_triage` (redesign 9.10) over a scripted backend: what makes
//! a mission stuck, the two closed questions, shadow and assist, reuse for
//! a week, and the plan's check that triage never completes a mission or
//! sets Verified. No test reaches TypeSafe.

use super::mission_triage::*;
use super::*;
use crate::service::view_scope::ViewScope;
use crate::service::work::graph::{AttemptBrief, GraphNode};
use crate::service::work::missions::{self, MissionDetail, MissionInput};
use crate::service::work::WorkLinkArgs;
use crate::store::{DecisionRunFilter, MissionEventRow, MissionRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Result<JevResponse, BackendError>>>,
    calls: AtomicUsize,
}

impl Fake {
    fn answering(answers: Vec<Result<JevResponse, BackendError>>) -> Arc<Fake> {
        Arc::new(Fake {
            script: Mutex::new(answers.into()),
            ..Default::default()
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl DecisionBackend for Fake {
    fn provider(&self) -> &'static str {
        PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        _req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(BackendError::Transport("script ran out".into())))
    }
}

/// A choice answer, all the probability on `choice`'s side.
fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == UNSURE { "retry" } else { UNSURE };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 90, "output_tokens": 2 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    mission: MissionRow,
    /// The clock the questions are asked on.
    now: Arc<AtomicI64>,
}

fn world() -> World {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let m = missions::save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("Ship login".into()),
                goal: Some("Passwordless login".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &store,
        &ViewScope::internal(),
    )
    .unwrap();
    let mission = {
        let s = store.lock().unwrap();
        s.set_mission_state(m.id, None, "active", "t").unwrap();
        s.get_mission(m.id).unwrap().unwrap()
    };
    World {
        store,
        mission,
        now: Arc::new(AtomicI64::new(crate::service::catalog::now_secs())),
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        let now = Arc::clone(&self.now);
        DecideCtx::new(Arc::clone(&self.store), fake.clone())
            .with_clock(Arc::new(move || now.load(Ordering::SeqCst)))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_MISSION_TRIAGE, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn runs(&self) -> Vec<crate::store::DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn detail(&self) -> MissionDetail {
        missions::mission(&self.store, &ViewScope::internal(), self.mission.id, None).unwrap()
    }
}

fn failing() -> Stuck {
    Stuck {
        reason: "failed".into(),
        why: "1 task failed".into(),
        done: 2,
        failed: 1,
        blocked: 0,
        total: 3,
        last_failure: Some("tests fail on token expiry".into()),
    }
}

fn node(item_id: i64, state: &str, attempt: Option<AttemptBrief>) -> GraphNode {
    GraphNode {
        item_id,
        state: state.into(),
        wave: 1,
        depends_on: vec![],
        waiting_for: vec![],
        verification: None,
        attempt,
    }
}

fn attempt(task_id: i64, error: &str) -> AttemptBrief {
    AttemptBrief {
        task_id,
        role: None,
        attempt: Some(1),
        state: "failed".into(),
        outcome: None,
        summary: Some("a summary".into()),
        error: Some(error.into()),
        evidence: None,
    }
}

fn event(at: i64, kind: &str, why: &str) -> MissionEventRow {
    MissionEventRow {
        id: at,
        at,
        kind: kind.into(),
        actor: "loop".into(),
        work_item_id: None,
        task_id: None,
        decision_id: None,
        payload: Some(serde_json::json!({ "why": why })),
    }
}

/// Fleet's own facts decide whether there is a card: a brake while the
/// mission stays paused, a failed item, or nothing ready with something
/// blocked. A finished mission is never stuck.
#[test]
fn a_stuck_mission_is_read_from_fleets_own_facts() {
    let w = world();
    let mut d = w.detail();
    assert_eq!(stuck(&d), None, "a fresh mission is not stuck");

    d.graph.nodes = vec![
        node(1, "done", None),
        node(2, "failed", Some(attempt(4, "old error"))),
        node(
            3,
            "failed",
            Some(attempt(9, &"x".repeat(FAILURE_CHARS + 50))),
        ),
        node(5, "ready", None),
    ];
    let s = stuck(&d).expect("a failed item");
    assert_eq!(
        (s.reason.as_str(), s.why.as_str(), s.done, s.failed, s.total),
        ("failed", "2 tasks failed", 1, 2, 4)
    );
    assert_eq!(
        s.last_failure.map(|f| f.chars().count()),
        Some(FAILURE_CHARS),
        "the newest failure, cut"
    );

    d.graph.nodes = vec![node(1, "done", None), node(2, "blocked", None)];
    let s = stuck(&d).expect("nothing ready, one blocked");
    assert_eq!(
        (s.reason.as_str(), s.why.as_str()),
        ("blocked", "Nothing is ready; 1 task blocked")
    );

    d.graph.nodes = vec![node(1, "running", None)];
    d.mission.state = "paused".into();
    d.events = vec![event(
        d.mission.updated_at,
        "budget",
        "the workers spent $4.00 of the grant's $4.00",
    )];
    let s = stuck(&d).expect("braked and still paused");
    assert_eq!(s.reason, "budget");
    assert_eq!(
        s.why,
        "Paused on its budget: the workers spent $4.00 of the grant's $4.00"
    );
    // Changed since the brake (resumed and paused by hand): not stuck.
    d.events[0].at = d.mission.updated_at - 1;
    assert_eq!(stuck(&d), None);

    d.mission.state = "completed".into();
    d.graph.nodes = vec![node(2, "failed", None)];
    assert_eq!(stuck(&d), None, "a finished mission is never stuck");
}

#[test]
fn both_questions_are_closed_and_offer_unsure() {
    let w = world();
    for (req, options) in [
        (outcome_request(&w.mission, &failing()), &OUTCOMES[..]),
        (next_request(&w.mission, &failing()), &NEXT_STEPS[..]),
    ] {
        let Question::Choice { criteria, .. } = &req.question else {
            panic!("a choice");
        };
        let mut want: Vec<&str> = options.to_vec();
        want.push(UNSURE);
        want.sort();
        let mut got: Vec<&str> = criteria.keys().map(String::as_str).collect();
        got.sort();
        assert_eq!(got, want);
        assert_eq!(req.state["goal"], "Passwordless login");
        assert_eq!(req.state["tasks"]["failed"], 1);
        assert_eq!(req.state["last_failure"], "tests fail on token expiry");
    }
}

#[tokio::test]
async fn off_asks_nothing() {
    let w = world();
    let fake = Fake::answering(vec![]);
    let p = ask(&w.ctx(&fake), &w.mission, &failing()).await;
    assert_eq!(p, Proposals::default());
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_both_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("partial", 0.9), says("retry", 0.9)]);
    let p = ask(&w.ctx(&fake), &w.mission, &failing()).await;
    assert_eq!(p, Proposals::default());
    let runs = w.runs();
    let mut subjects: Vec<String> = runs.iter().map(|r| r.subject_id.clone()).collect();
    subjects.sort();
    let id = w.mission.id;
    assert_eq!(
        subjects,
        vec![format!("{id}:next"), format!("{id}:outcome")]
    );
    assert!(runs.iter().all(|r| r.subject_kind == SUBJECT_KIND));
}

/// The plan's 9.10 check: whatever Jev answers, triage changes nothing but
/// the decision log. A `done` outcome does not complete the mission, change
/// its state, or record a verification; a `give_up` does not cancel it.
#[tokio::test]
async fn triage_never_completes_a_mission_or_sets_verified() {
    let w = world();
    w.on("assist");
    let id = w.mission.id;
    let snapshot = || {
        let s = w.store.lock().unwrap();
        let items = s.mission_items(id).unwrap();
        let verified: Vec<_> = items
            .iter()
            .map(|i| s.latest_verifications(i.id).unwrap())
            .collect();
        (
            s.get_mission(id).unwrap().unwrap(),
            items,
            verified,
            s.mission_events(id, None, 100).unwrap(),
            s.mission_cards(id, 100).unwrap(),
        )
    };
    let before = snapshot();
    let fake = Fake::answering(vec![says("done", 0.97), says("give_up", 0.95)]);
    let p = ask(&w.ctx(&fake), &w.mission, &failing()).await;
    let outcome = p.outcome.expect("an outcome proposed");
    let next = p.next.expect("a next step proposed");
    assert_eq!(
        (outcome.value.as_str(), next.value.as_str()),
        ("done", "give_up")
    );
    assert_eq!(outcome.source, "jev");
    assert_eq!(outcome.confidence_pct, Some(97));
    assert_eq!(outcome.reason.as_deref(), Some("1 task failed"));
    assert_eq!(snapshot(), before, "nothing but the decision log changed");
    assert_eq!(before.0.state, "active");
}

#[tokio::test]
async fn unsure_or_too_unsure_proposes_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9), says("retry", 0.3)]);
    let p = ask(&w.ctx(&fake), &w.mission, &failing()).await;
    assert_eq!(p, Proposals::default());
    assert_eq!(fake.calls(), 2);
}

/// A decided run on the same input is reused for a week, its proposal
/// included; new facts or an older run ask again.
#[tokio::test]
async fn a_decided_run_is_reused_for_a_week() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("partial", 0.8),
        says("retry", 0.8),
        says("partial", 0.8),
        says("split", 0.8),
        says("failed", 0.8),
        says("give_up", 0.8),
    ]);
    let ctx = w.ctx(&fake);
    let first = ask(&ctx, &w.mission, &failing()).await;
    let again = ask(&ctx, &w.mission, &failing()).await;
    assert_eq!(fake.calls(), 2, "asked once");
    assert_eq!(first, again);
    assert_eq!(again.next.unwrap().value, "retry");

    let worse = Stuck {
        failed: 2,
        why: "2 tasks failed".into(),
        ..failing()
    };
    let p = ask(&ctx, &w.mission, &worse).await;
    assert_eq!(fake.calls(), 4, "new facts ask again");
    assert_eq!(p.next.unwrap().value, "split");

    w.now.fetch_add(REUSE_SECS + 1, Ordering::SeqCst);
    let p = ask(&ctx, &w.mission, &worse).await;
    assert_eq!(fake.calls(), 6, "a week later it asks again");
    assert_eq!(p.outcome.unwrap().value, "failed");
}
