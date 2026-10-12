//! J10 `restore_target` and N4 `adopt_target` over a scripted backend: the
//! question and state, the gate, assist (a proposal, or unsure and a blank
//! form), shadow, reuse of a decided run, and follow-ups. No test reaches
//! TypeSafe.

use super::lost_target::*;
use super::start_project::Candidate;
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
    let other = if choice == UNSURE { "p2" } else { UNSURE };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 60, "output_tokens": 2 },
    }))
    .unwrap())
}

fn cand(project_id: i64, repo: &str) -> Candidate {
    Candidate {
        project_id,
        owner: "acme".into(),
        repo: repo.into(),
    }
}

struct World {
    store: Arc<Mutex<Store>>,
}

fn world() -> World {
    World {
        store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOON))
    }
    fn on(&self, feature: Feature, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, feature.setting_key(), mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn pane(&self) -> LostInput {
        LostInput {
            kind: LostKind::Pane,
            subject: pane_subject(42),
            org_id: None,
            cwd: "/home/ada/scratch/receipt-card".into(),
            git_branch: None,
            name: Some("fleet-trn-scratch".into()),
            candidates: vec![cand(1, "papaya-pos"), cand(2, "payments-api")],
        }
    }
    fn transcript(&self) -> LostInput {
        let s = self.store.lock().unwrap();
        let fp = s.decision_fp_key().unwrap();
        LostInput {
            kind: LostKind::Transcript,
            subject: transcript_subject(&fp, "44366faf-ae97-426a-91cd-beaf3c74f1d7"),
            org_id: None,
            cwd: "/home/ada/tmp/pd-2412".into(),
            git_branch: Some("pd-2412-receipt-totals".into()),
            name: None,
            candidates: vec![cand(1, "papaya-pos"), cand(2, "payments-api")],
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

#[test]
fn the_question_and_state_name_the_place_and_repositories_only() {
    let w = world();
    let input = w.transcript();
    let req = question_for(&input);
    assert_eq!(req, question_for(&input), "pure");
    let Question::Choice { criteria, .. } = &req.question else {
        panic!("a choice");
    };
    let options: Vec<&str> = criteria.keys().map(String::as_str).collect();
    assert_eq!(options, vec!["p1", "p2", UNSURE]);
    assert_eq!(
        req.state,
        serde_json::json!({
            "place": {
                "cwd": "/home/ada/tmp/pd-2412",
                "branch": "pd-2412-receipt-totals",
                "name": "",
            },
            "candidates": ["acme/papaya-pos", "acme/payments-api"],
        })
    );
    assert_eq!(
        reason_for(&input),
        "directory and branch pd-2412-receipt-totals"
    );
    assert_eq!(
        reason_for(&w.pane()),
        "directory and the name fleet-trn-scratch"
    );
}

#[test]
fn a_transcript_subject_never_records_the_id() {
    let w = world();
    let subject = w.transcript().subject;
    assert!(subject.starts_with("t:"), "{subject}");
    assert!(!subject.contains("44366faf"), "{subject}");
    assert_eq!(subject.len(), 2 + 16);
    assert_eq!(pane_subject(42), "session:42");
}

#[tokio::test]
async fn off_by_default_nothing_is_asked_and_nothing_prefilled() {
    let w = world();
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    let t = propose(&w.ctx(&fake), w.pane()).await;
    assert_eq!(t, LostTarget::default());
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty(), "nothing recorded while off");
}

#[tokio::test]
async fn assist_prefills_the_answer_with_who_and_why() {
    let w = world();
    w.on(Feature::AdoptTarget, "assist");
    let fake = Fake::answering(vec![says("p1", 0.82)]);
    let t = propose(&w.ctx(&fake), w.pane()).await;
    assert_eq!(t.project_id, Some(1));
    assert_eq!(t.source.as_deref(), Some("jev"));
    assert_eq!(t.confidence_pct, Some(82));
    assert_eq!(
        t.reason.as_deref(),
        Some("directory and the name fleet-trn-scratch")
    );
    assert!(!t.unsure);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].feature, "adopt_target");
    assert_eq!(runs[0].subject_kind, "lost_pane");
    assert_eq!(runs[0].subject_id, "session:42");
    assert_eq!(runs[0].question_version, "adopt_target.v1");
    assert_eq!(Some(runs[0].id), t.run_id);
}

#[tokio::test]
async fn unsure_or_low_confidence_leaves_the_form_blank_and_says_so() {
    let w = world();
    w.on(Feature::RestoreTarget, "assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9)]);
    let t = propose(&w.ctx(&fake), w.transcript()).await;
    assert_eq!(t.project_id, None, "unsure prefills nothing");
    assert!(t.unsure);

    let w = world();
    w.on(Feature::RestoreTarget, "assist");
    let fake = Fake::answering(vec![says("p2", 0.3)]);
    let t = propose(&w.ctx(&fake), w.transcript()).await;
    assert_eq!(t.project_id, None, "below the floor prefills nothing");
    assert!(t.unsure);
}

#[tokio::test]
async fn shadow_records_and_prefills_nothing() {
    let w = world();
    w.on(Feature::RestoreTarget, "shadow");
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    let t = ask(&w.ctx(&fake), &w.transcript()).await;
    assert_eq!(t, LostTarget::default(), "shadow never prefills");
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, "shadow");
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(UNSURE));
}

#[tokio::test]
async fn one_feature_on_does_not_ask_for_the_other() {
    let w = world();
    w.on(Feature::AdoptTarget, "assist");
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    let t = propose(&w.ctx(&fake), w.transcript()).await;
    assert_eq!(t, LostTarget::default());
    assert_eq!(fake.calls(), 0, "restore_target is still off");
}

#[tokio::test]
async fn the_same_input_reuses_the_decided_run() {
    let w = world();
    w.on(Feature::AdoptTarget, "assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    let first = propose(&w.ctx(&fake), w.pane()).await;
    let again = propose(&w.ctx(&fake), w.pane()).await;
    assert_eq!(fake.calls(), 1, "asked once");
    assert_eq!(again.project_id, Some(2));
    assert_eq!(again.run_id, first.run_id);
}

#[tokio::test]
async fn a_persons_choice_marks_the_proposal_they_were_shown() {
    let w = world();
    w.on(Feature::AdoptTarget, "assist");
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    let t = propose(&w.ctx(&fake), w.pane()).await;
    let mark = |chosen| {
        let s = w.store.lock().unwrap();
        record_choice(&s, LostKind::Pane, &pane_subject(42), chosen, NOON).unwrap()
    };
    assert!(mark(Some(1)));
    let run = &w.runs()[0];
    assert_eq!(Some(run.id), t.run_id);
    assert_eq!(run.followup.as_deref(), Some("confirmed"));
    assert!(!mark(Some(2)), "a decided run is never marked twice");

    for (chosen, want, theirs) in [(Some(2), "corrected", Some("p2")), (None, "rejected", None)] {
        let w = world();
        w.on(Feature::AdoptTarget, "assist");
        let fake = Fake::answering(vec![says("p1", 0.9)]);
        propose(&w.ctx(&fake), w.pane()).await;
        let s = w.store.lock().unwrap();
        assert!(record_choice(&s, LostKind::Pane, &pane_subject(42), chosen, NOON).unwrap());
        drop(s);
        let run = &w.runs()[0];
        assert_eq!(run.followup.as_deref(), Some(want));
        assert_eq!(run.corrected_to.as_deref(), theirs);
    }
}

#[tokio::test]
async fn a_shadow_answer_nobody_saw_is_never_marked() {
    let w = world();
    w.on(Feature::AdoptTarget, "shadow");
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    ask(&w.ctx(&fake), &w.pane()).await;
    let s = w.store.lock().unwrap();
    assert!(!record_choice(&s, LostKind::Pane, &pane_subject(42), Some(2), NOON).unwrap());
}
