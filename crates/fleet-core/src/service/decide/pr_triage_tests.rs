//! PR shepherd step 4, `pr_triage`, over a scripted backend: off asks
//! nothing, shadow records the answer beside the shepherd's own choice, and
//! only the PR's reading is sent. No test reaches TypeSafe.

use super::pr_triage::*;
use super::*;
use crate::service::outcome::{CheckSummary, FailingCheck, PrEvidence};
use crate::service::pr_shepherd::Condition;
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
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: 0.8, "needs_person": 0.2 },
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
        settings::set(&s, settings::DECIDE_JEV_PR_TRIAGE, mode).unwrap();
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

fn red() -> PrEvidence {
    PrEvidence {
        head_oid: Some("a".repeat(40)),
        merge_state: Some("UNSTABLE".into()),
        state: Some("OPEN".into()),
        checks: CheckSummary {
            total: 5,
            failing: vec![FailingCheck {
                name: "rust (ubuntu-24.04)".into(),
                url: Some("https://github.com/o/r/actions/runs/1/job/2".into()),
            }],
            failing_total: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn off_asks_nothing() {
    let (ctx, fake) = setup("off", vec![says("flaky_rerun")]);
    assert_eq!(triage(&ctx, 7, None, Condition::CiRed, &red()).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(runs(&ctx).is_empty());
}

#[tokio::test]
async fn shadow_records_the_answer_beside_the_shepherds_choice() {
    let (ctx, fake) = setup("shadow", vec![says("not_this_pr")]);
    let got = triage(&ctx, 7, None, Condition::CiRed, &red()).await;
    assert_eq!(got.as_deref(), Some("not_this_pr"));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let r = runs(&ctx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].feature, "pr_triage");
    assert_eq!(r[0].subject_id, "7");
    assert_eq!(r[0].answer.as_deref(), Some("not_this_pr"));
    assert_eq!(r[0].baseline_answer.as_deref(), Some("fix_in_pr"));
    assert_eq!(r[0].candidates.len(), OPTIONS.len());
}

#[tokio::test]
async fn only_the_reading_is_sent_and_never_a_url() {
    let (ctx, fake) = setup("shadow", vec![says("fix_in_pr")]);
    triage(&ctx, 7, None, Condition::CiRed, &red()).await;
    let seen = fake.seen.lock().unwrap();
    let state = &seen[0].state;
    assert_eq!(state["condition"], "ci_red");
    assert_eq!(state["merge_state"], "UNSTABLE");
    assert_eq!(state["failing_checks"][0], "rust (ubuntu-24.04)");
    assert!(!state.to_string().contains("https://"), "{state}");
}

#[tokio::test]
async fn a_failed_call_is_recorded_and_answers_nothing() {
    let (ctx, _) = setup("shadow", vec![Err(BackendError::Transport("down".into()))]);
    assert_eq!(
        triage(&ctx, 7, None, Condition::Conflict, &red()).await,
        None
    );
    let r = runs(&ctx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].baseline_answer.as_deref(), Some("merge_base"));
    assert!(r[0].fallback.is_some());
}

#[test]
fn the_question_is_a_valid_closed_set_and_the_rule_picks_from_it() {
    assert!(question().check().is_ok());
    for c in [Condition::Conflict, Condition::Behind, Condition::CiRed] {
        assert!(is_option(rule_choice(c)));
    }
    let many = PrEvidence {
        checks: CheckSummary {
            failing: (0..20)
                .map(|i| FailingCheck {
                    name: format!("c{i}"),
                    url: None,
                })
                .collect(),
            failing_total: 20,
            ..Default::default()
        },
        ..Default::default()
    };
    let r = request(Condition::CiRed, &many);
    assert_eq!(
        r.state["failing_checks"].as_array().unwrap().len(),
        CHECK_NAMES_MAX
    );
    assert_eq!(r.state["failing_total"], 20);
}
