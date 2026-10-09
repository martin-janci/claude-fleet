//! J9 `summary_check` (redesign 11.11) over a scripted backend: off asks
//! nothing, shadow records and shows, assist shows only a supported summary
//! and hides one it cannot check. No test reaches TypeSafe.

use super::summary_check::*;
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

fn noul(v: f64) -> Result<JevResponse, BackendError> {
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": { "type": "noul", "noul": v } },
        "usage": { "input_tokens": 400, "output_tokens": 1 },
    }))
    .unwrap())
}

fn setup(mode: &str, answers: Vec<Result<JevResponse, BackendError>>) -> (DecideCtx, Arc<Fake>) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_SUMMARY_CHECK, mode).unwrap();
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

#[tokio::test]
async fn off_asks_nothing_and_the_summary_shows_unchecked() {
    let (ctx, fake) = setup("off", vec![noul(0.0)]);
    let c = check(&ctx, 7, None, "Fixed the test.", "user: fix it").await;
    assert_eq!(c, Check::Off);
    assert!(c.shows());
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(runs(&ctx).is_empty());
}

#[tokio::test]
async fn shadow_records_and_still_shows() {
    let (ctx, fake) = setup("shadow", vec![noul(0.1)]);
    let c = check(&ctx, 7, None, "Fixed the test.", "user: fix it").await;
    assert_eq!(c, Check::Shadow);
    assert!(c.shows(), "shadow never hides");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let r = runs(&ctx);
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].feature.as_str(), r[0].subject_id.as_str()),
        ("summary_check", "7")
    );
}

/// The plan's 11.11 check: a failed J9 check hides the summary.
#[tokio::test]
async fn assist_hides_an_unsupported_summary_and_shows_a_supported_one() {
    let (ctx, fake) = setup("assist", vec![noul(0.92), noul(0.2)]);
    let ok = check(
        &ctx,
        7,
        None,
        "Fixed the test.",
        "assistant: fixed the test",
    )
    .await;
    assert_eq!(ok, Check::Passed);
    assert!(ok.shows());
    let bad = check(&ctx, 7, None, "Merged the PR.", "assistant: fixed the test").await;
    assert_eq!(bad, Check::Failed);
    assert!(!bad.shows());
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen[1].state["summary"], "Merged the PR.");
    assert_eq!(seen[1].state["transcript"], "assistant: fixed the test");
}

#[tokio::test]
async fn assist_hides_a_summary_it_could_not_check() {
    let (ctx, _) = setup("assist", vec![Err(BackendError::Transport("down".into()))]);
    let c = check(&ctx, 7, None, "Fixed the test.", "x").await;
    assert_eq!(c, Check::Unchecked);
    assert!(!c.shows());
    // An org that has not consented is refused by the gate: hidden too.
    let (ctx, fake) = setup("assist", vec![noul(0.9)]);
    let org = ctx
        .store
        .lock()
        .unwrap()
        .add_org("Acme", None, false)
        .unwrap()
        .id;
    assert_eq!(
        check(&ctx, 7, Some(org), "Fixed the test.", "x").await,
        Check::Unchecked
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_transcript_sent_is_its_newest_part() {
    assert_eq!(tail_bytes("abc", 10), "abc");
    assert_eq!(tail_bytes("abcdef", 3), "def");
    // Never a cut inside a character.
    assert_eq!(tail_bytes("aé", 1), "");
    let long = "x".repeat(TRANSCRIPT_MAX_BYTES + 100);
    let r = request("s", &long);
    assert_eq!(
        r.state["transcript"].as_str().unwrap().len(),
        TRANSCRIPT_MAX_BYTES
    );
    assert!(r.question.check().is_ok());
}
