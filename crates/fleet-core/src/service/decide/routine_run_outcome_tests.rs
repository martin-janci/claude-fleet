//! N6 `routine_run_outcome` over a scripted backend and a scripted pane:
//! the defaults, both consents, assist and shadow, one decision per run,
//! the exit and the rules always winning, the follow-up, and what is sent.
//! No test reaches TypeSafe or tmux.

use super::routine_run_outcome::*;
use super::*;
use crate::service::routines::outcome::{self as run_outcome, OutcomeSource, RunOutcome};
use crate::store::{DecisionRunFilter, DecisionRunRow, NewRoutineRun, RoutineFields, SessionRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
const NOW: i64 = 1_791_417_600;

/// An idle REPL after a sweep that found nothing.
const NOTHING: &str = "⏺ I checked your open pull requests: none has new reviews or failing \
                       checks. Nothing to do today.\n\n────────────────\n❯ \n────────────────\n  \
                       ⏵⏵ accept edits on (shift+tab to cycle) · ? for shortcuts";

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

fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == UNSURE { "did_work" } else { UNSURE };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 120, "output_tokens": 2 },
    }))
    .unwrap())
}

/// A pane that always shows `screen`, counting the reads.
struct Pane {
    screen: String,
    reads: AtomicUsize,
}

#[async_trait::async_trait]
impl PaneReader for Pane {
    async fn tail(&self, _row: &SessionRow) -> Result<String, IpcError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.screen.clone())
    }
}

fn pane(screen: &str) -> Pane {
    Pane {
        screen: screen.into(),
        reads: AtomicUsize::new(0),
    }
}

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
    session: i64,
    run: i64,
}

/// A routine of org Acme whose run finished a minute ago on a session of a
/// host of Acme, nothing answered yet (no rule reads a plain finished turn).
fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.upsert_host("hosta").unwrap();
    s.set_host_org("hosta", Some(org)).unwrap();
    let project = s.upsert_project("acme", "web", "/src").unwrap();
    let session = s
        .upsert_session("rt-0", "hosta", Some(project), None, 1, 1, "running", None)
        .unwrap();
    let routine = s
        .insert_routine(
            Some(org),
            None,
            &RoutineFields {
                name: "Morning PR sweep".into(),
                enabled: true,
                trigger: "manual".into(),
                host_alias: "hosta".into(),
                project_id: project,
                prompt: "Review my open PRs.".into(),
                overlap: "skip".into(),
                ..Default::default()
            },
            None,
            0,
        )
        .unwrap();
    let run = s
        .insert_routine_run(&NewRoutineRun {
            routine_id: routine.id,
            trigger: "run_now",
            state: "running",
            session_id: Some(session),
            at: NOW - 600,
            ..Default::default()
        })
        .unwrap()
        .id;
    s.finish_routine_run(run, "done", None, 0, NOW - 60)
        .unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        org,
        session,
        run,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOW))
    }
    /// The kill switch, the mode, a key and both consents (D31 and D48).
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_ROUTINE_RUN_OUTCOME, mode).unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_org_jev_reply_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn pending(&self) -> Vec<RunInput> {
        pending(&self.store.lock().unwrap(), NOW).unwrap()
    }
    fn outcome(&self) -> (Option<String>, Option<String>) {
        let r = self
            .store
            .lock()
            .unwrap()
            .get_routine_run(self.run)
            .unwrap()
            .unwrap();
        (r.outcome, r.outcome_source)
    }
    fn session(&self) -> SessionRow {
        self.store
            .lock()
            .unwrap()
            .get_session_by_id(self.session)
            .unwrap()
            .unwrap()
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
}

fn pair(o: &str, src: &str) -> (Option<String>, Option<String>) {
    (Some(o.into()), Some(src.into()))
}

#[tokio::test]
async fn with_the_defaults_nothing_is_read_or_asked() {
    let w = world();
    assert!(w.pending().is_empty());
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    let p = pane(NOTHING);
    read_pending(&w.ctx(&fake), &p).await;
    assert_eq!(p.reads.load(Ordering::SeqCst), 0);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
    assert_eq!(w.outcome(), (None, None));
}

/// It sends Claude's reply text, so D31 alone is not enough (D48).
#[tokio::test]
async fn the_run_screen_needs_the_reply_text_consent_too() {
    let w = world();
    w.on("assist");
    w.store
        .lock()
        .unwrap()
        .set_org_jev_reply_allowed(w.org, false)
        .unwrap();
    assert!(Feature::RoutineRunOutcome.sends_reply_text());
    assert!(w.pending().is_empty());
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    read_pending(&w.ctx(&fake), &pane(NOTHING)).await;
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn assist_nothing_to_do_keeps_the_run_out_of_the_inbox_once() {
    let w = world();
    w.on("assist");
    assert_eq!(w.pending().len(), 1);
    assert_eq!(w.session().last_viewed_at, None);
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    let ctx = w.ctx(&fake);
    read_pending(&ctx, &pane(NOTHING)).await;
    assert_eq!(fake.calls(), 1);
    assert_eq!(w.outcome(), pair("nothing", "jev"));
    assert_eq!(
        w.session().last_viewed_at,
        Some(NOW),
        "seen, so its finished turn is not unread"
    );
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].subject_kind, SUBJECT_KIND);
    assert_eq!(runs[0].subject_id, w.run.to_string());
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(BASELINE));
    // Answered: never read again.
    assert!(w.pending().is_empty());
    read_pending(&ctx, &pane(NOTHING)).await;
    assert_eq!(fake.calls(), 1);
    // What was sent: the reply, the chrome left out, and the four options.
    let sent = fake.seen.lock().unwrap()[0].clone();
    let screen = sent.state["screen"].as_str().unwrap();
    assert!(screen.contains("Nothing to do today."), "{screen}");
    assert!(!screen.contains("shift+tab"), "{screen}");
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec!["did_work", "needs_person", "nothing", "unsure"],
    );
}

#[tokio::test]
async fn shadow_unsure_or_a_weak_answer_applies_nothing_and_asks_once() {
    for (mode, answer) in [
        ("shadow", says("nothing", 0.9)),
        ("assist", says(UNSURE, 0.9)),
        ("assist", says("nothing", 0.3)),
    ] {
        let w = world();
        w.on(mode);
        let fake = Fake::answering(vec![answer]);
        let input = w.pending().remove(0);
        assert_eq!(
            ask(&w.ctx(&fake), &input, NOTHING).await,
            Asked::Recorded,
            "{mode}"
        );
        assert_eq!(w.outcome(), (None, None), "{mode}");
        assert_eq!(w.session().last_viewed_at, None, "{mode}");
        assert_eq!(w.runs().len(), 1);
        assert!(w.pending().is_empty(), "decided: {mode}");
    }
}

#[tokio::test]
async fn old_runs_answered_runs_and_runs_without_a_pane_are_not_read() {
    // Finished more than an hour ago: its screen has moved on.
    let w = world();
    w.on("assist");
    assert!(pending(&w.store.lock().unwrap(), NOW + RECENT_SECS)
        .unwrap()
        .is_empty());
    // A rule answered it.
    let w = world();
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        let run = s.get_routine_run(w.run).unwrap().unwrap();
        run_outcome::record(&s, &run, RunOutcome::DidWork, OutcomeSource::Rule, NOW).unwrap();
    }
    assert!(w.pending().is_empty());
    // Its session was lost (its host went away).
    let w = world();
    w.on("assist");
    w.store
        .lock()
        .unwrap()
        .mark_host_sessions_lost("hosta", "host offline", &[], NOW, NOW)
        .unwrap();
    assert!(w.session().lost_at.is_some());
    assert!(w.pending().is_empty());
}

/// The exit and the rules always win: an answer that arrives after a rule
/// answered is only recorded, and a rule that answers after Jev replaces
/// it and is its follow-up.
#[tokio::test]
async fn the_rules_win_over_jev_before_and_after_its_answer() {
    // Before: a rule answers while Jev is asked.
    let w = world();
    w.on("assist");
    let input = w.pending().remove(0);
    {
        let s = w.store.lock().unwrap();
        let run = s.get_routine_run(w.run).unwrap().unwrap();
        run_outcome::record(&s, &run, RunOutcome::NeedsPerson, OutcomeSource::Rule, NOW).unwrap();
    }
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &input, NOTHING).await, Asked::Recorded);
    assert_eq!(w.outcome(), pair("needs_person", "rule"));
    assert_eq!(w.session().last_viewed_at, None);

    // After: Jev said nothing, then the run's pull request turns up.
    let w = world();
    w.on("assist");
    let input = w.pending().remove(0);
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    let got = ask(&w.ctx(&fake), &input, NOTHING).await;
    assert!(matches!(got, Asked::Applied { ref outcome, .. } if outcome == "nothing"));
    {
        let s = w.store.lock().unwrap();
        let run = s.get_routine_run(w.run).unwrap().unwrap();
        assert!(
            run_outcome::record(&s, &run, RunOutcome::DidWork, OutcomeSource::Rule, NOW + 5)
                .unwrap()
        );
    }
    assert_eq!(w.outcome(), pair("did_work", "rule"));
    let runs = w.runs();
    assert_eq!(runs[0].followup.as_deref(), Some("corrected"));
    assert_eq!(runs[0].corrected_to.as_deref(), Some("did_work"));
}

#[tokio::test]
async fn a_shadow_answer_is_never_marked() {
    let w = world();
    w.on("shadow");
    let input = w.pending().remove(0);
    let fake = Fake::answering(vec![says("nothing", 0.9)]);
    ask(&w.ctx(&fake), &input, NOTHING).await;
    let s = w.store.lock().unwrap();
    assert!(!record_rule(&s, w.run, "did_work", NOW).unwrap());
    drop(s);
    assert_eq!(w.runs()[0].followup, None);
}

#[test]
fn every_option_but_unsure_is_an_outcome_the_store_takes() {
    let Question::Choice { criteria, .. } = question() else {
        panic!("a choice");
    };
    for k in criteria.keys() {
        match applied(k) {
            Some(o) => assert!(crate::store::ROUTINE_RUN_OUTCOMES.contains(&o.as_str())),
            None => assert_eq!(k, UNSURE),
        }
    }
    assert_eq!(
        Feature::parse("routine_run_outcome"),
        Some(Feature::RoutineRunOutcome)
    );
    assert_eq!(
        Feature::RoutineRunOutcome.setting_key(),
        settings::DECIDE_JEV_ROUTINE_RUN_OUTCOME
    );
}
