//! K1 `start_project` over a scripted backend: the gate, assist and shadow,
//! reuse of a decided run, follow-ups, and no key text in the record. No
//! test reaches TypeSafe.

use super::start_project::*;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
/// Noon, UTC.
const NOON: i64 = 1_790_510_400;

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Result<JevResponse, BackendError>>>,
    calls: AtomicUsize,
    seen: Mutex<Vec<JevRequest>>,
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
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(req.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(BackendError::Transport("script ran out".into())))
    }
}

/// A choice answer, all the probability on `choice`'s side.
fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == UNSURE { "p1" } else { UNSURE };
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
    org: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    World {
        store: Arc::new(Mutex::new(s)),
        org,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOON))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_START_PROJECT, mode).unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn input(&self) -> StartInput {
        StartInput {
            key: "PAY-12".into(),
            title: "Refund button on the POS checkout screen".into(),
            item_id: Some(7),
            org_id: Some(self.org),
            description: Some("The cashier needs a refund action next to Pay.".into()),
            candidates: vec![
                Candidate {
                    project_id: 1,
                    owner: "acme".into(),
                    repo: "backoffice".into(),
                },
                Candidate {
                    project_id: 2,
                    owner: "acme".into(),
                    repo: "pos-frontend".into(),
                },
            ],
        }
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    let ctx = w.ctx(&fake);
    assert_eq!(mode_for(&ctx, &w.input()), None);
    assert_eq!(ask(&ctx, &w.input()).await, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn assist_suggests_a_candidate_and_reuses_the_decided_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    let ctx = w.ctx(&fake);
    assert_eq!(mode_for(&ctx, &w.input()), Some(Mode::Assist));
    let got = ask(&ctx, &w.input()).await.expect("a suggestion");
    assert_eq!((got.project_id, got.confidence_pct), (2, Some(90)));
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(got.run_id, Some(runs[0].id));
    assert_eq!(runs[0].subject_id, "item:7");
    assert_eq!(
        runs[0].baseline_answer.as_deref(),
        Some("p1"),
        "today's first row"
    );
    // The popover opened again on the same task: no second call.
    let again = ask(&ctx, &w.input()).await.expect("reused");
    assert_eq!(again.project_id, 2);
    assert_eq!(fake.calls(), 1);
    // What was sent: the task's text and the repositories, nothing else.
    let sent = fake.seen.lock().unwrap()[0].clone();
    assert_eq!(sent.state["task"]["key"], "PAY-12");
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec!["p1", "p2", UNSURE]
    );
}

#[tokio::test]
async fn unsure_or_a_weak_answer_suggests_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.3)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    assert_eq!(w.runs()[0].fallback.as_deref(), Some("low_confidence"));
}

#[tokio::test]
async fn shadow_records_and_suggests_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].mode.as_str(), runs[0].answer.as_deref()),
        ("shadow", Some("p2"))
    );
    // A person's start never marks an answer nobody saw.
    let s = w.store.lock().unwrap();
    assert!(!record_start(&s, Some(7), "PAY-12", 1, NOON).unwrap());
}

#[tokio::test]
async fn a_persons_start_confirms_or_corrects_the_proposal_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    ask(&w.ctx(&fake), &w.input()).await.unwrap();
    {
        let s = w.store.lock().unwrap();
        assert!(record_start(&s, Some(7), "PAY-12", 1, NOON).unwrap());
        assert!(
            !record_start(&s, Some(7), "PAY-12", 2, NOON).unwrap(),
            "a follow-up is never overwritten"
        );
    }
    let r = &w.runs()[0];
    assert_eq!(
        (r.followup.as_deref(), r.corrected_to.as_deref()),
        (Some("corrected"), Some("p1"))
    );

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    ask(&w.ctx(&fake), &w.input()).await.unwrap();
    let s = w.store.lock().unwrap();
    assert!(record_start(&s, Some(7), "PAY-12", 2, NOON).unwrap());
    drop(s);
    assert_eq!(w.runs()[0].followup.as_deref(), Some("confirmed"));
}

#[tokio::test]
async fn a_key_no_tracker_knows_is_recorded_only_as_a_fingerprint() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p1", 0.8)]);
    let input = StartInput {
        item_id: None,
        key: "SECRET-PROJECT-9".into(),
        ..w.input()
    };
    ask(&w.ctx(&fake), &input).await.unwrap();
    let subject = w.runs()[0].subject_id.clone();
    assert!(subject.starts_with("key:"), "{subject}");
    assert!(!subject.contains("SECRET"), "{subject}");
    let s = w.store.lock().unwrap();
    assert!(record_start(&s, None, "SECRET-PROJECT-9", 1, NOON).unwrap());
}

#[tokio::test]
async fn no_candidates_asks_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![]);
    let input = StartInput {
        candidates: vec![],
        ..w.input()
    };
    assert_eq!(ask(&w.ctx(&fake), &input).await, None);
    assert_eq!(fake.calls(), 0);
}

/// Step 2.8: a start's pre-selection is also a proposal in the one shape
/// every row carries.
#[test]
fn a_suggested_project_is_a_start_project_proposal() {
    let p = SuggestedProject {
        project_id: 12,
        confidence_pct: Some(76),
        run_id: Some(3),
    }
    .proposal();
    assert_eq!(
        (p.feature.as_str(), p.value.as_str(), p.source.as_str()),
        ("start_project", "p12", "jev")
    );
    assert_eq!((p.confidence_pct, p.run_id), (Some(76), Some(3)));
    assert_eq!(project_of(&p.value), Some(12));
}
