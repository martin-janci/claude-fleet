//! `restore_timing` over a scripted backend: off asks nothing, shadow
//! records beside the rule and is not followed, assist is followed, a
//! repeated question is reused rather than asked every tick, and only
//! bucketed numbers are sent. No test reaches TypeSafe.

use super::restore_timing::*;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Result<JevResponse, BackendError>>>,
    calls: AtomicUsize,
    seen: Mutex<Vec<JevRequest>>,
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

fn says(choice: &str) -> Result<JevResponse, BackendError> {
    let other = if choice == DEFER { RECOVER_NOW } else { DEFER };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: 0.8, other: 0.2 },
            "confidence": 0.8,
        }},
        "usage": { "input_tokens": 120, "output_tokens": 2 },
    }))
    .unwrap())
}

fn setup(mode: &str, answers: Vec<Result<JevResponse, BackendError>>) -> (DecideCtx, Arc<Fake>) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_RESTORE_TIMING, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    let fake = Arc::new(Fake {
        script: Mutex::new(answers.into()),
        ..Default::default()
    });
    let ctx = DecideCtx::new(store, fake.clone()).with_clock(Arc::new(crate::store::now_unix));
    (ctx, fake)
}

fn runs(ctx: &DecideCtx) -> Vec<DecisionRunRow> {
    ctx.store
        .lock()
        .unwrap()
        .list_decision_runs(&DecisionRunFilter::default())
        .unwrap()
}

fn q() -> RestoreQuestion {
    RestoreQuestion {
        session_id: 7,
        org_id: None,
        lost_at: crate::store::now_unix() - 120,
        lost_reason: "process_died".into(),
        idle_minutes: minutes_bucket(400),
        turn_outcome: Some("working".into()),
        mid_turn: true,
        failed_attempts: 0,
        host: HostFacts {
            alias: "secret-box".into(),
            cpu_busy_pct: Some(tenth(97.0)),
            mem_avail_pct: Some(tenth(4.0)),
            disk_free_pct: Some(tenth(55.0)),
            live_sessions: 6,
        },
        move_to: vec![HostFacts {
            alias: "calm".into(),
            cpu_busy_pct: Some(10),
            mem_avail_pct: Some(60),
            disk_free_pct: Some(70),
            live_sessions: 1,
        }],
        baseline: DEFER,
    }
}

#[tokio::test]
async fn off_asks_nothing_and_records_nothing() {
    let (ctx, fake) = setup("off", vec![says(RECOVER_NOW)]);
    assert_eq!(ask(&ctx, &q()).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(runs(&ctx).is_empty());
}

#[tokio::test]
async fn shadow_records_beside_the_rule_and_is_not_followed() {
    let (ctx, fake) = setup("shadow", vec![says(RECOVER_NOW)]);
    let a = ask(&ctx, &q()).await.unwrap();
    assert_eq!(a.advice, Advice::RecoverNow);
    assert!(!a.follow);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let r = runs(&ctx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].feature, "restore_timing");
    assert_eq!(r[0].subject_id, "7");
    assert_eq!(r[0].baseline_answer.as_deref(), Some(DEFER));
}

#[tokio::test]
async fn assist_is_followed_and_a_move_names_an_offered_host() {
    let (ctx, _) = setup("assist", vec![says("m:calm")]);
    let a = ask(&ctx, &q()).await.unwrap();
    assert_eq!(a.advice, Advice::Move("calm".into()));
    assert!(a.follow);
}

#[tokio::test]
async fn the_same_question_is_reused_not_asked_every_tick() {
    let (ctx, fake) = setup("assist", vec![says(DEFER), says(RECOVER_NOW)]);
    let first = ask(&ctx, &q()).await.unwrap();
    let again = ask(&ctx, &q()).await.unwrap();
    assert_eq!(first.advice, Advice::Defer);
    assert_eq!(again.advice, Advice::Defer);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    // A failing backend is not asked again within REASK_SECS either.
    let (ctx, fake) = setup("assist", vec![]);
    assert_eq!(ask(&ctx, &q()).await, None);
    assert_eq!(ask(&ctx, &q()).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn only_bucketed_numbers_are_sent_never_a_name() {
    let (ctx, fake) = setup("shadow", vec![says(DEFER)]);
    ask(&ctx, &q()).await;
    let seen = fake.seen.lock().unwrap();
    let state = &seen[0].state;
    assert_eq!(state["host"]["cpu_busy_pct"], 90);
    assert_eq!(state["host"]["mem_avail_pct"], 0);
    assert_eq!(state["session"]["idle_minutes"], 15);
    assert_eq!(state["session"]["mid_turn"], true);
    assert_eq!(state["hosts"]["m:calm"]["mem_avail_pct"], 60);
    assert!(!state.to_string().contains("secret-box"), "{state}");
}

#[test]
fn the_question_is_closed_and_the_bounds_hold() {
    let q = q();
    assert!(question(&q).check().is_ok());
    assert_eq!(advice_of("m:calm", &q), Some(Advice::Move("calm".into())));
    assert_eq!(advice_of("m:elsewhere", &q), None);
    assert_eq!(bounded(Advice::RecoverNow, true, &q), Some(Advice::Defer));
    assert_eq!(
        bounded(Advice::RecoverNow, false, &q),
        Some(Advice::RecoverNow)
    );
    assert_eq!(bounded(Advice::Move("gone".into()), false, &q), None);
    assert_eq!(minutes_bucket(0), 5);
    assert_eq!(minutes_bucket(3 * 3600), 240);
    assert_eq!(minutes_bucket(30 * 86_400), 10_080);
    assert!(reason_line(&Advice::Defer, Some(0.8)).ends_with("80% sure"));
}
