//! `limit_switch` over a scripted backend: off asks nothing, one candidate
//! asks nothing, shadow records the answer beside the rule's pick, and only
//! bucketed numbers are sent. No test reaches TypeSafe.

use super::limit_switch::*;
use super::*;
use crate::service::account_limits::HostLogin;
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
    let other = if choice == "own" { "p:work" } else { "own" };
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
        settings::set(&s, settings::DECIDE_JEV_LIMIT_SWITCH, mode).unwrap();
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

fn login(profile: Option<&str>, uuid: &str, used: f64) -> HostLogin {
    HostLogin {
        profile: profile.map(str::to_string),
        account_uuid: uuid.to_string(),
        used_pct: Some(used),
    }
}

fn two() -> Vec<HostLogin> {
    vec![
        login(None, "acct-secret-own", 37.9),
        login(Some("work"), "acct-secret-work", 12.0),
    ]
}

#[tokio::test]
async fn off_asks_nothing() {
    let (ctx, fake) = setup("off", vec![says("own")]);
    let c = two();
    assert_eq!(shadow(&ctx, 7, None, &c, &c[1]).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(runs(&ctx).is_empty());
}

#[tokio::test]
async fn one_candidate_decides_itself() {
    let (ctx, fake) = setup("shadow", vec![says("own")]);
    let c = vec![login(None, "a", 10.0)];
    assert_eq!(shadow(&ctx, 7, None, &c, &c[0]).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn shadow_records_the_answer_beside_the_rules_pick() {
    let (ctx, fake) = setup("shadow", vec![says("own")]);
    let c = two();
    assert_eq!(
        shadow(&ctx, 7, None, &c, &c[1]).await.as_deref(),
        Some("own")
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let r = runs(&ctx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].feature, "limit_switch");
    assert_eq!(r[0].subject_id, "7");
    assert_eq!(r[0].answer.as_deref(), Some("own"));
    assert_eq!(r[0].baseline_answer.as_deref(), Some("p:work"));
}

#[tokio::test]
async fn only_bucketed_numbers_are_sent_never_an_account() {
    let (ctx, fake) = setup("shadow", vec![says("p:work")]);
    let c = two();
    shadow(&ctx, 7, None, &c, &c[1]).await;
    let seen = fake.seen.lock().unwrap();
    let state = &seen[0].state;
    assert_eq!(state["logins"]["own"]["used_pct"], 30);
    assert_eq!(state["logins"]["own"]["machine_login"], true);
    assert_eq!(state["logins"]["p:work"]["used_pct"], 10);
    assert!(!state.to_string().contains("acct-secret"), "{state}");
}

#[test]
fn the_question_is_a_valid_closed_set() {
    assert!(question(&two()).check().is_ok());
    assert_eq!(option_of(&two()[0]), "own");
    assert_eq!(option_of(&two()[1]), "p:work");
}
