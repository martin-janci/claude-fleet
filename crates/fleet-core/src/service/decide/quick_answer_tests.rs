//! J5 `quick_answer` over a scripted backend: what may be asked (never a
//! permission, a multi-select, or a risky option), assist and shadow, reuse,
//! withdrawal, and a form's proposal by value. No test reaches TypeSafe.

use super::quick_answer::*;
use super::*;
use crate::service::pane_intel::{PendingInput, PendingOption};
use crate::store::{DecisionRunFilter, DecisionRunRow};
use serde_json::json;
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
    let other = if choice == UNSURE { "o1" } else { UNSURE };
    Ok(serde_json::from_value(json!({
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

fn pending(kind: &str, labels: &[&str]) -> PendingInput {
    PendingInput {
        kind: kind.into(),
        question: Some("Which test runner should I use for the new package?".into()),
        options: labels
            .iter()
            .enumerate()
            .map(|(i, l)| PendingOption {
                n: (i + 1) as u8,
                label: (*l).into(),
                selected: i == 0,
                checked: false,
            })
            .collect(),
        multi: false,
        detail: None,
    }
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
        settings::set(&s, settings::DECIDE_JEV_QUICK_ANSWER, mode).unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn input(&self) -> QuickInput {
        session_input(
            4,
            Some(self.org),
            &pending("input", &["Vitest", "Jest", "Type something."]),
        )
        .expect("an askable question")
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
fn a_push_a_permission_or_a_step_hard_to_undo_is_never_proposed() {
    for l in [
        "Yes, push to origin/main",
        "Force-push the branch",
        "Delete the worktree",
        "git reset --hard",
        "Deploy to production",
        "Yes, and don't ask again for this command",
        "Yes, and don’t ask again for: ls *",
        "Always allow edits",
        "Merge the PR",
    ] {
        assert!(risky(l), "{l}");
    }
    for l in [
        "Vitest",
        "Keep the current name",
        "Use tabs",
        "Pushover notifications",
    ] {
        assert!(!risky(l), "{l}");
    }
    // A permission dialog is never asked about, whatever its options.
    assert_eq!(
        session_input(4, None, &pending("permission", &["Yes", "No"])),
        None
    );
    // Nor a multi-select one (a digit only ticks a box there).
    let mut multi = pending("input", &["A", "B", "C"]);
    multi.multi = true;
    assert_eq!(session_input(4, None, &multi), None);
    // A risky option is left out of the question; fewer than two safe ones
    // and nothing is asked.
    let q = session_input(
        4,
        None,
        &pending("input", &["Push now", "Wait", "Open a draft PR"]),
    )
    .unwrap();
    assert_eq!(
        q.choices.iter().map(|c| c.n).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(q.baseline, None, "the cursor sits on the risky option");
    assert_eq!(
        session_input(4, None, &pending("input", &["Push now", "Wait"])),
        None
    );
    // Ten options: past what 1–9 answer.
    let ten: Vec<String> = (1..=10).map(|i| format!("Option {i}")).collect();
    let ten: Vec<&str> = ten.iter().map(String::as_str).collect();
    assert_eq!(session_input(4, None, &pending("input", &ten)), None);
}

#[test]
fn a_question_that_names_a_risky_action_asks_nothing() {
    // "Yes, go ahead" names no push, but the question does: answering it
    // pushes, so no option of it is proposed (F10).
    for q in [
        "Push the 3 commits to origin/main now?",
        "Merge the PR into main?",
        "Deploy the build to staging?",
        "Delete the old worktree?",
        "Approve the plan and start?",
        "Run rm on the cache dir?",
    ] {
        let mut p = pending("input", &["Not yet", "Yes, go ahead"]);
        p.question = Some(q.into());
        assert!(risky_question(q), "{q}");
        assert_eq!(session_input(4, None, &p), None, "{q}");
    }
    // An ordinary question still asks.
    let p = pending("input", &["Not yet", "Yes, go ahead"]);
    assert!(session_input(4, None, &p).is_some());
    // "Approve" as an option is a person's too.
    assert!(risky("Approve"));
}

#[test]
fn the_ts_mirror_checks_the_same_words() {
    let ts = crate::repo_files::read("src/lib/quick_answer.ts");
    for w in RISKY_WORDS {
        assert!(ts.contains(&format!("'{w}'")), "quick_answer.ts misses {w}");
    }
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("o1", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn assist_proposes_a_safe_option_and_reuses_the_decided_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("o2", 0.8)]);
    let ctx = w.ctx(&fake);
    let got = ask(&ctx, &w.input()).await.expect("a proposal");
    assert_eq!((got.value.as_str(), got.confidence_pct), ("o2", Some(80)));
    let runs = w.runs();
    assert_eq!(
        (runs[0].subject_kind.as_str(), runs[0].subject_id.as_str()),
        (SUBJECT_SESSION, "4")
    );
    assert_eq!(
        runs[0].baseline_answer.as_deref(),
        Some("o1"),
        "the cursor's option"
    );
    assert_eq!(ask(&ctx, &w.input()).await.unwrap().value, "o2");
    assert_eq!(fake.calls(), 1, "the same question is not asked twice");
    // What was sent: the question and the safe options, never the free text.
    let sent = fake.seen.lock().unwrap()[0].clone();
    assert!(sent.state["question"]
        .as_str()
        .unwrap()
        .contains("test runner"));
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec!["o1", "o2", UNSURE]
    );
}

#[tokio::test]
async fn unsure_a_weak_or_an_unoffered_answer_proposes_nothing() {
    for answer in [says(UNSURE, 0.9), says("o2", 0.3), says("o3", 0.9)] {
        let w = world();
        w.on("assist");
        let fake = Fake::answering(vec![answer]);
        assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    }
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("o2", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    let r = &w.runs()[0];
    assert_eq!(
        (r.mode.as_str(), r.answer.as_deref()),
        ("shadow", Some("o2"))
    );
}

#[tokio::test]
async fn a_question_that_went_away_withdraws_its_proposal() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("o2", 0.9), says("o1", 0.9)]);
    let ctx = w.ctx(&fake);
    ask(&ctx, &w.input()).await.unwrap();
    {
        let s = w.store.lock().unwrap();
        assert_eq!(withdraw(&s, SUBJECT_SESSION, "4", NOON), 1);
        assert_eq!(withdraw(&s, SUBJECT_SESSION, "4", NOON), 0, "once");
    }
    assert_eq!(w.runs()[0].followup.as_deref(), Some("ignored"));
    // The same question back again is asked again: a withdrawn run is not
    // the subject's proposal any more.
    assert_eq!(ask(&ctx, &w.input()).await.unwrap().value, "o1");
    assert_eq!(fake.calls(), 2);
}

#[test]
fn a_sessions_question_is_the_subject_its_row_carries_proposals_for() {
    assert_eq!(SUBJECT_SESSION, crate::store::PROPOSAL_SUBJECT_SESSION);
}

#[test]
fn a_questions_digest_ignores_the_cursor_and_moves_with_its_words() {
    let a = pending("input", &["Vitest", "Jest"]);
    let mut moved = a.clone();
    moved.options[0].selected = false;
    moved.options[1].selected = true;
    assert_eq!(digest(&a), digest(&moved));
    assert_ne!(digest(&a), digest(&pending("input", &["Vitest", "Mocha"])));
}

fn spec() -> serde_json::Value {
    json!({
        "spec": "fleet.form/1",
        "title": "Deploy",
        "steps": [{ "title": "Target", "fields": [
            { "name": "env", "type": "select", "label": "Environment", "value": "stg",
              "options": [["stg", "Staging"], ["prod", "Production"], ["qa", "QA"]] },
            { "name": "note", "type": "text", "label": "Note" }
        ]}]
    })
}

#[test]
fn a_forms_first_choice_is_asked_about_by_number_and_never_on_a_risky_value() {
    let fc = form_choice("f_a", None, "Deploy", Some("to pick a target"), &spec()).unwrap();
    assert_eq!(fc.field, "env");
    // "Production" (and its value "prod") is risky: left out.
    assert_eq!(
        fc.input.choices.iter().map(|c| c.n).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(fc.input.baseline, Some(1), "the field's default");
    assert!(fc.input.question.contains("to pick a target"));
    // Two choices on the first step, or a step with a condition: nothing.
    let mut two = spec();
    two["steps"][0]["fields"][1] = json!({ "name": "zone", "type": "select", "label": "Zone", "options": [["a", "A"], ["b", "B"]] });
    assert_eq!(form_choice("f_a", None, "Deploy", None, &two), None);
    let mut cond = spec();
    cond["steps"][0]["when"] = json!({ "field": "x", "truthy": true });
    assert_eq!(form_choice("f_a", None, "Deploy", None, &cond), None);
}

#[tokio::test]
async fn a_forms_proposal_names_the_options_value() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("o3", 0.7)]);
    let fc = form_choice("f_a", Some(w.org), "Deploy", None, &spec()).unwrap();
    ask(&w.ctx(&fake), &fc.input).await.unwrap();
    let s = w.store.lock().unwrap();
    let p = form_proposal(&s, &fc).expect("a proposal");
    assert_eq!(
        (
            p.field.as_str(),
            p.value.as_str(),
            p.source.as_str(),
            p.confidence_pct
        ),
        ("env", "qa", "jev", Some(70))
    );
    withdraw(&s, SUBJECT_FORM, "f_a", NOON);
    assert_eq!(form_proposal(&s, &fc), None);
}
