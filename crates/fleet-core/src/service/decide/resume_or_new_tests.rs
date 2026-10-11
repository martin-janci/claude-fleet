//! N2 `resume_or_new` over a scripted backend: the rule, the question and
//! state, the gate, assist (resume, new, or unsure), shadow, reuse of a
//! decided run, follow-ups, and the store-backed path from a key's ended
//! links. No test reaches TypeSafe.

use super::resume_or_new::*;
use super::*;
use crate::service::view_scope::ViewScope;
use crate::store::{DecisionRunFilter, DecisionRunRow, WorkTarget};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
/// Noon, UTC.
const NOON: i64 = 1_790_510_400;
const DAY: i64 = 86_400;

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
    let other = if choice == UNSURE { NEW } else { UNSURE };
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

fn past(link_id: i64, name: &str, ended_days_ago: i64) -> PastSession {
    PastSession {
        link_id,
        session_id: Some(link_id + 100),
        name: name.into(),
        branch: Some(format!("pd-2412-{name}")),
        ended_at: NOON - ended_days_ago * DAY,
        role: "work".into(),
        has_pr: link_id == 7,
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
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, Feature::ResumeOrNew.setting_key(), mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn input(&self, past_sessions: Vec<PastSession>) -> ResumeInput {
        let s = self.store.lock().unwrap();
        let fp = s.decision_fp_key().unwrap();
        ResumeInput {
            key: "PD-2412".into(),
            subject: subject_id(&fp, "PD-2412"),
            org_id: None,
            recent_days: 14,
            past: past_sessions,
        }
    }
    /// Two past sessions: the rule leaves it to Jev.
    fn two(&self) -> ResumeInput {
        self.input(vec![
            past(7, "receipt-totals", 2),
            past(5, "receipt-card", 9),
        ])
    }
    fn subject(&self) -> String {
        self.two().subject
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
fn the_rule_resumes_the_only_recent_past_session_and_leaves_the_rest_to_jev() {
    let w = world();
    let one = w.input(vec![past(7, "receipt-totals", 2)]);
    let r = rule(&one, NOON).expect("one recent past session decides");
    assert_eq!(r.value.as_deref(), Some("l7"));
    assert_eq!(r.link_id, Some(7));
    assert_eq!(r.session_id, Some(107));
    assert_eq!(r.name.as_deref(), Some("receipt-totals"));
    assert_eq!(r.source.as_deref(), Some("rule"));
    assert_eq!(
        r.reason.as_deref(),
        Some("the only past session, ended 2 days ago")
    );
    assert_eq!(r.confidence_pct, None, "a rule has no confidence");
    assert!(r.run_id.is_none());

    let old = w.input(vec![past(7, "receipt-totals", 30)]);
    assert_eq!(rule(&old, NOON), None, "ended long ago: Jev's");
    assert_eq!(rule(&w.two(), NOON), None, "two past sessions: Jev's");
    assert_eq!(rule(&w.input(vec![]), NOON), None);
}

#[test]
fn the_question_and_state_name_the_key_and_past_sessions_only() {
    let w = world();
    let input = w.two();
    let req = question_for(&input, NOON);
    assert_eq!(req, question_for(&input, NOON), "pure");
    let Question::Choice { criteria, .. } = &req.question else {
        panic!("a choice");
    };
    let mut options: Vec<&str> = criteria.keys().map(String::as_str).collect();
    options.sort();
    assert_eq!(options, vec!["l5", "l7", NEW, UNSURE]);
    assert_eq!(
        req.state,
        serde_json::json!({
            "key": "PD-2412",
            "past": [
                {
                    "option": "l7", "name": "receipt-totals",
                    "branch": "pd-2412-receipt-totals", "ended_days_ago": 2,
                    "role": "work", "pull_request": true,
                },
                {
                    "option": "l5", "name": "receipt-card",
                    "branch": "pd-2412-receipt-card", "ended_days_ago": 9,
                    "role": "work", "pull_request": false,
                },
            ],
        })
    );
}

#[test]
fn a_subject_never_records_the_key() {
    let w = world();
    let subject = w.subject();
    assert!(subject.starts_with("key:"), "{subject}");
    assert!(!subject.to_lowercase().contains("pd-2412"), "{subject}");
    assert_eq!(subject.len(), 4 + 16);
}

#[tokio::test]
async fn off_by_default_nothing_is_asked_and_only_the_rule_answers() {
    let w = world();
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(t, ResumeOrNew::default());
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty(), "nothing recorded while off");

    let t = propose(&w.ctx(&fake), w.input(vec![past(7, "receipt-totals", 1)])).await;
    assert_eq!(t.source.as_deref(), Some("rule"), "a rule is not AI");
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn assist_never_asks_jev_when_the_rule_decides() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(NEW, 0.9)]);
    let t = propose(&w.ctx(&fake), w.input(vec![past(7, "receipt-totals", 1)])).await;
    assert_eq!(t.source.as_deref(), Some("rule"));
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn assist_proposes_a_resume_with_who_and_why() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("l5", 0.82)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(t.value.as_deref(), Some("l5"));
    assert_eq!(t.link_id, Some(5));
    assert_eq!(t.session_id, Some(105));
    assert_eq!(t.name.as_deref(), Some("receipt-card"));
    assert_eq!(t.source.as_deref(), Some("jev"));
    assert_eq!(t.confidence_pct, Some(82));
    assert_eq!(
        t.reason.as_deref(),
        Some("its name, branch and ended 9 days ago")
    );
    assert!(!t.unsure);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].feature, "resume_or_new");
    assert_eq!(runs[0].subject_kind, "work_resume");
    assert_eq!(runs[0].subject_id, w.subject());
    assert_eq!(runs[0].question_version, "resume_or_new.v1");
    assert_eq!(Some(runs[0].id), t.run_id);
}

#[tokio::test]
async fn assist_may_propose_starting_fresh() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(NEW, 0.75)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(t.value.as_deref(), Some(NEW));
    assert_eq!(t.link_id, None);
    assert_eq!(t.source.as_deref(), Some("jev"));
    assert!(!t.unsure);
}

#[tokio::test]
async fn unsure_or_low_confidence_proposes_nothing_and_says_so() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(t.value, None, "unsure proposes nothing");
    assert!(t.unsure);

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("l7", 0.3)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(t.value, None, "below the floor proposes nothing");
    assert!(t.unsure);
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    let t = ask(&w.ctx(&fake), &w.two()).await;
    assert_eq!(t, ResumeOrNew::default(), "shadow never proposes");
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, "shadow");
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(UNSURE));
}

#[tokio::test]
async fn the_same_input_reuses_the_decided_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    let first = propose(&w.ctx(&fake), w.two()).await;
    let again = propose(&w.ctx(&fake), w.two()).await;
    assert_eq!(fake.calls(), 1, "asked once");
    assert_eq!(again.value.as_deref(), Some("l7"));
    assert_eq!(again.run_id, first.run_id);
}

#[tokio::test]
async fn a_persons_choice_marks_the_proposal_they_were_shown() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    let t = propose(&w.ctx(&fake), w.two()).await;
    let subject = w.subject();
    let mark = |chosen: &str| {
        let s = w.store.lock().unwrap();
        record_choice(&s, &subject, chosen, NOON).unwrap()
    };
    assert!(mark("l7"));
    let run = &w.runs()[0];
    assert_eq!(Some(run.id), t.run_id);
    assert_eq!(run.followup.as_deref(), Some("confirmed"));
    assert!(!mark(NEW), "a decided run is never marked twice");

    // "Start fresh instead" corrects a resume to `new`.
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    propose(&w.ctx(&fake), w.two()).await;
    let subject = w.subject();
    {
        let s = w.store.lock().unwrap();
        assert!(record_choice(&s, &subject, NEW, NOON).unwrap());
    }
    let run = &w.runs()[0];
    assert_eq!(run.followup.as_deref(), Some("corrected"));
    assert_eq!(run.corrected_to.as_deref(), Some(NEW));
}

#[tokio::test]
async fn a_shadow_or_unsure_answer_is_never_marked() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("l7", 0.9)]);
    ask(&w.ctx(&fake), &w.two()).await;
    let subject = w.subject();
    {
        let s = w.store.lock().unwrap();
        assert!(!record_choice(&s, &subject, NEW, NOON).unwrap());
    }

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9)]);
    propose(&w.ctx(&fake), w.two()).await;
    let subject = w.subject();
    let s = w.store.lock().unwrap();
    assert!(!record_choice(&s, &subject, NEW, NOON).unwrap());
}

/// A past session of PD-2412: linked, then killed, so its link ended.
fn ended_session(s: &Store, tmux: &str) -> i64 {
    s.upsert_host("mercury").unwrap();
    let id = s
        .upsert_session(tmux, "mercury", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(id, WorkTarget::Key("PD-2412"), "manual")
        .unwrap();
    s.delete_session(id).unwrap();
    s.ended_work_links_for_key("PD-2412")
        .unwrap()
        .iter()
        .find(|l| l.snap_tmux.as_deref() == Some(tmux))
        .unwrap()
        .id
}

#[tokio::test]
async fn a_keys_ended_links_feed_the_rule_and_the_follow_up_checks_the_choice() {
    let w = world();
    let link = {
        let s = w.store.lock().unwrap();
        ended_session(&s, "dev-o-r--pd-2412")
    };
    let ctx = DecideCtx::new(Arc::clone(&w.store), Fake::answering(vec![]))
        .with_clock(Arc::new(crate::store::now_unix));
    let view = ViewScope::internal();
    let t = propose_for_key(&ctx, &view, "pd-2412").await.unwrap();
    assert_eq!(t.value, Some(format!("l{link}")));
    assert_eq!(t.link_id, Some(link));
    assert_eq!(t.source.as_deref(), Some("rule"));
    assert_eq!(t.name.as_deref(), Some("dev-o-r--pd-2412"));

    // A key with no past work proposes nothing.
    let none = propose_for_key(&ctx, &view, "PD-9999").await.unwrap();
    assert_eq!(none, ResumeOrNew::default());

    // The follow-up takes `new` or one of the key's past sessions only;
    // with no Jev proposal there is nothing to mark.
    let now = crate::store::now_unix();
    assert!(!follow_for_key(&w.store, &view, "PD-2412", NEW, now).unwrap());
    assert!(!follow_for_key(&w.store, &view, "PD-2412", &format!("l{link}"), now).unwrap());
    let e = follow_for_key(&w.store, &view, "PD-2412", "l999999", now).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
    let e = follow_for_key(&w.store, &view, "PD-2412", "resume", now).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
}
