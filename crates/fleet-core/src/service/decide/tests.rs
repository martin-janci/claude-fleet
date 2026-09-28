//! The envelope's tests: the gate matrix, recording on every path, the
//! breaker, the budget, answer checks, redaction and the fingerprint, and
//! the key never showing. No test reaches TypeSafe: [`Fake`] answers, or
//! [`JevBackend`] over a scripted [`FakeTransport`].

use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::store::DecisionRunFilter;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

/// A scripted backend: answers in order, records what it was asked.
#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Result<JevResponse, BackendError>>>,
    calls: AtomicUsize,
    seen: Mutex<Vec<(String, String, JevRequest)>>,
    delay: Option<Duration>,
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
        key: &Secret,
        model: &str,
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push((key.expose().to_string(), model.to_string(), req.clone()));
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(BackendError::Transport("script ran out".into())))
    }
}

fn choice_response(choice: &str, confidence: f64, tokens: u64) -> JevResponse {
    serde_json::from_value(json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { "ACME-1": confidence, "ACME-2": 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": tokens, "output_tokens": 3 },
    }))
    .unwrap()
}

fn choice_request() -> JevRequest {
    JevRequest {
        state: json!({
            "prompt": "fix the login redirect, see https://acme.atlassian.net/browse/ACME-1?token=abc123 and mail bob@acme.io",
        }),
        question: Question::Choice {
            instructions: json!("Which ticket is this session working on?"),
            criteria: BTreeMap::from([
                ("ACME-1".to_string(), Some(json!("Login redirect loops"))),
                ("ACME-2".to_string(), Some(json!("Billing export"))),
            ]),
        },
    }
}

fn request(feature: Feature, org_id: Option<i64>) -> DecideRequest {
    DecideRequest {
        feature,
        subject_kind: "session".into(),
        subject_id: "42".into(),
        org_id,
        request: choice_request(),
        baseline: Some("ACME-2".into()),
        question_version: "wl.1".into(),
        min_confidence: None,
    }
}

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
    clock: Arc<AtomicI64>,
}

/// Noon, UTC.
const NOON: i64 = 1_790_510_400;

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    World {
        store: Arc::new(Mutex::new(s)),
        org,
        clock: Arc::new(AtomicI64::new(NOON)),
    }
}

impl World {
    fn ctx(&self, backend: Arc<dyn DecisionBackend>) -> DecideCtx {
        let c = Arc::clone(&self.clock);
        DecideCtx::new(Arc::clone(&self.store), backend)
            .with_clock(Arc::new(move || c.load(Ordering::SeqCst)))
    }
    fn set(&self, key: &str, value: &str) {
        settings::set(&self.store.lock().unwrap(), key, value).unwrap();
    }
    /// Everything on for `work_link` in shadow, the org consenting, a key.
    fn all_on(&self) {
        self.set(settings::DECIDE_JEV_ENABLED, "true");
        self.set(settings::DECIDE_JEV_WORK_LINK, "shadow");
        let s = self.store.lock().unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn gate(&self, f: Feature, org: Option<i64>) -> Result<Mode, Fallback> {
        gate_at(
            &self.store.lock().unwrap(),
            f,
            org,
            self.clock.load(Ordering::SeqCst),
        )
    }
    fn runs(&self) -> Vec<crate::store::DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
}

// --- the gate -----------------------------------------------------------------

#[test]
fn with_the_defaults_the_gate_refuses_everything() {
    let w = world();
    for f in Feature::ALL {
        for org in [None, Some(w.org)] {
            assert_eq!(w.gate(f, org), Err(Fallback::FlagOff), "{f:?} {org:?}");
        }
    }
    // Even with a key and the org consenting: the kill switch is off.
    {
        let s = w.store.lock().unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
        s.set_org_jev_allowed(w.org, true).unwrap();
    }
    assert_eq!(
        w.gate(Feature::WorkLink, Some(w.org)),
        Err(Fallback::FlagOff)
    );
}

#[tokio::test]
async fn with_the_defaults_no_call_is_made_and_the_refusal_is_recorded() {
    let w = world();
    let fake = Fake::answering(vec![Ok(choice_response("ACME-1", 0.9, 10))]);
    let out = decide(
        &w.ctx(fake.clone()),
        request(Feature::WorkLink, Some(w.org)),
    )
    .await;
    assert_eq!(fake.calls(), 0);
    assert_eq!(out.fallback, Some(Fallback::FlagOff));
    assert_eq!(out.mode, None);
    assert!(out.answer.is_none() && out.usable().is_none());
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(Some(r.id), out.run_id);
    assert_eq!(r.fallback.as_deref(), Some("flag_off"));
    assert_eq!(r.mode, "off");
    assert!(!r.called);
    assert_eq!(r.candidates, vec!["ACME-1", "ACME-2"]);
    assert_eq!(r.baseline_answer.as_deref(), Some("ACME-2"));
    assert!(r.input_fp.is_some(), "coverage rows are fingerprinted too");
}

#[test]
fn the_gate_matrix() {
    let w = world();
    let f = Feature::WorkLink;
    w.set(settings::DECIDE_JEV_ENABLED, "true");
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::ModeOff));
    w.set(settings::DECIDE_JEV_WORK_LINK, "shadow");
    // The other feature keeps its own mode.
    assert_eq!(
        w.gate(Feature::StatusMap, Some(w.org)),
        Err(Fallback::ModeOff)
    );
    // No consent: the org's, or for no org the global bool.
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::OrgOff));
    assert_eq!(w.gate(f, None), Err(Fallback::OrgOff));
    assert_eq!(w.gate(f, Some(9_999)), Err(Fallback::OrgOff), "unknown org");
    w.store
        .lock()
        .unwrap()
        .set_org_jev_allowed(w.org, true)
        .unwrap();
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::NoKey));
    assert_eq!(
        w.gate(f, None),
        Err(Fallback::OrgOff),
        "an org's consent is its own"
    );
    w.store
        .lock()
        .unwrap()
        .set_decision_credential(Some(&Secret::new(KEY)), None)
        .unwrap();
    assert_eq!(w.gate(f, Some(w.org)), Ok(Mode::Shadow));
    w.set(settings::DECIDE_JEV_WORK_LINK, "assist");
    assert_eq!(w.gate(f, Some(w.org)), Ok(Mode::Assist));
    w.set(settings::DECIDE_JEV_UNASSIGNED, "true");
    assert_eq!(w.gate(f, None), Ok(Mode::Assist));
    // A key reference that cannot be read is no key.
    w.store
        .lock()
        .unwrap()
        .set_decision_credential(None, Some("env:FLEET_TEST_UNSET_JEV_KEY_VAR"))
        .unwrap();
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::NoKey));
    w.store
        .lock()
        .unwrap()
        .set_decision_credential(Some(&Secret::new(KEY)), None)
        .unwrap();
    // The kill switch wins over everything.
    w.set(settings::DECIDE_JEV_ENABLED, "false");
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::FlagOff));
    w.set(settings::DECIDE_JEV_ENABLED, "true");
    // Revoking the org's consent stops it at once.
    w.store
        .lock()
        .unwrap()
        .set_org_jev_allowed(w.org, false)
        .unwrap();
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::OrgOff));
}

#[test]
fn a_desktop_paired_to_a_hub_never_calls_out() {
    let w = world();
    w.all_on();
    assert_eq!(w.gate(Feature::WorkLink, Some(w.org)), Ok(Mode::Shadow));
    w.store
        .lock()
        .unwrap()
        .set_setting(HUB_REMOTE_URL_KEY, "https://hub.example.com")
        .unwrap();
    assert!(!owns_the_fleet(&w.store.lock().unwrap()));
    assert_eq!(
        w.gate(Feature::WorkLink, Some(w.org)),
        Err(Fallback::NotOwner)
    );
    w.store
        .lock()
        .unwrap()
        .set_setting(HUB_REMOTE_URL_KEY, "  ")
        .unwrap();
    assert_eq!(w.gate(Feature::WorkLink, Some(w.org)), Ok(Mode::Shadow));
}

#[test]
fn org_consent_goes_through_the_org_admin_path() {
    use crate::service::orgs::{admin, OrgAction};
    use crate::service::trackers::admin::WorkAdminArgs;
    let w = world();
    let s = w.store.lock().unwrap();
    assert!(
        !s.get_org(w.org).unwrap().unwrap().jev_allowed,
        "off by default"
    );
    let args = |jev: &str| WorkAdminArgs {
        action: "update_org".into(),
        org_id: Some(w.org),
        jev: Some(jev.into()),
        ..Default::default()
    };
    let v = admin(OrgAction::UpdateOrg, &args("on"), &s).unwrap();
    assert_eq!(v["jev_allowed"], json!(true));
    assert!(s.org_jev_allowed(w.org).unwrap());
    assert!(admin(OrgAction::UpdateOrg, &args("maybe"), &s).is_err());
    admin(OrgAction::UpdateOrg, &args("off"), &s).unwrap();
    assert!(!s.org_jev_allowed(w.org).unwrap());
    let added = admin(
        OrgAction::AddOrg,
        &WorkAdminArgs {
            action: "add_org".into(),
            name: Some("Beta".into()),
            jev: Some("on".into()),
            ..Default::default()
        },
        &s,
    )
    .unwrap();
    assert_eq!(added["jev_allowed"], json!(true));
    // Another update that does not name it leaves it alone.
    let id = added["id"].as_i64().unwrap();
    admin(
        OrgAction::UpdateOrg,
        &WorkAdminArgs {
            action: "update_org".into(),
            org_id: Some(id),
            name: Some("Beta 2".into()),
            ..Default::default()
        },
        &s,
    )
    .unwrap();
    assert!(s.org_jev_allowed(id).unwrap());
    assert!(WorkAdminArgs {
        jev: Some("on".into()),
        ..Default::default()
    }
    .audit_summary()
    .contains("jev=Some(\"on\")"));
}

// --- a call -------------------------------------------------------------------

#[tokio::test]
async fn a_shadow_call_records_ids_numbers_and_a_fingerprint() {
    let w = world();
    w.all_on();
    let fake = Fake::answering(vec![Ok(choice_response("ACME-1", 0.8, 1_200))]);
    let out = decide(
        &w.ctx(fake.clone()),
        request(Feature::WorkLink, Some(w.org)),
    )
    .await;
    assert_eq!(fake.calls(), 1);
    assert_eq!(out.mode, Some(Mode::Shadow));
    assert_eq!(out.fallback, None);
    let a = out.usable().unwrap();
    assert_eq!(a.value, "ACME-1");
    assert_eq!(a.confidence, Some(0.8));
    assert!(out.proposal().is_none(), "shadow proposes nothing");
    let (key, model, sent) = fake.seen.lock().unwrap()[0].clone();
    assert_eq!(key, KEY);
    assert_eq!(model, "jev-1.13.0", "pinned by default");
    let r = &w.runs()[0];
    assert_eq!(Some(r.id), out.run_id);
    assert_eq!(r.mode, "shadow");
    assert_eq!(r.provider, "jev");
    assert_eq!(r.model_version.as_deref(), Some("jev-1.13.0"));
    assert_eq!(r.question_version, "wl.1");
    assert_eq!(r.answer.as_deref(), Some("ACME-1"));
    assert_eq!(r.probabilities.as_ref().unwrap()["ACME-2"], 1.0 - 0.8);
    assert_eq!(r.confidence, Some(0.8));
    assert!(r.fallback.is_none());
    assert!(r.called);
    assert!(r.latency_ms.is_some());
    assert_eq!(r.input_tokens, 1_200);
    assert_eq!(r.cost_microusd, 51, "1200 tokens at $0.042/M, rounded up");
    assert_eq!(r.org_id, Some(w.org));
    assert_eq!(
        (r.subject_kind.as_str(), r.subject_id.as_str()),
        ("session", "42")
    );
    // The fingerprint is the HMAC of what was sent, under the local key.
    let fp_key = w.store.lock().unwrap().decision_fp_key().unwrap();
    assert_eq!(
        r.input_fp.as_deref(),
        Some(fingerprint(&fp_key, &sent).as_str())
    );
    // No text of the request reached the row.
    let row = serde_json::to_string(r).unwrap();
    for text in ["login", "redirect", "Billing", "acme.io", "session working"] {
        assert!(!row.contains(text), "{text} leaked into {row}");
    }
}

#[tokio::test]
async fn assist_proposes_and_low_confidence_is_recorded_but_not_usable() {
    let w = world();
    w.all_on();
    w.set(settings::DECIDE_JEV_WORK_LINK, "assist");
    let fake = Fake::answering(vec![
        Ok(choice_response("ACME-2", 0.95, 10)),
        Ok(choice_response("ACME-1", 0.6, 10)),
    ]);
    let ctx = w.ctx(fake.clone());
    let mut req = request(Feature::WorkLink, Some(w.org));
    req.min_confidence = Some(0.75);
    let out = decide(&ctx, req.clone()).await;
    assert_eq!(out.proposal().unwrap().value, "ACME-2");
    let out = decide(&ctx, req).await;
    assert_eq!(out.fallback, Some(Fallback::LowConfidence));
    assert_eq!(out.answer.as_ref().unwrap().value, "ACME-1");
    assert!(out.usable().is_none() && out.proposal().is_none());
    let r = &w.runs()[0];
    assert_eq!(r.fallback.as_deref(), Some("low_confidence"));
    assert_eq!(r.answer.as_deref(), Some("ACME-1"), "kept for calibration");
}

#[tokio::test]
async fn an_answer_that_does_not_fit_the_question_is_invalid() {
    let w = world();
    w.all_on();
    let bad = |v: serde_json::Value| -> Result<JevResponse, BackendError> {
        Ok(serde_json::from_value(json!({
            "model": "jev-1.13.0", "answers": { "q": v }, "usage": { "input_tokens": 5 }
        }))
        .unwrap())
    };
    let fake = Fake::answering(vec![
        // An option that was not offered.
        bad(json!({"type": "choice", "choice": "ACME-3", "confidence": 0.9})),
        // Probabilities that are not a distribution.
        bad(json!({"type": "choice", "choice": "ACME-1",
                   "probabilities": {"ACME-1": 0.9, "ACME-2": 0.9}, "confidence": 0.9})),
        // A probability for an option not offered.
        bad(json!({"type": "choice", "choice": "ACME-1",
                   "probabilities": {"ACME-1": 0.5, "ZZZ-9": 0.5}, "confidence": 0.9})),
        // The wrong type.
        bad(json!({"type": "noul", "noul": 0.9})),
        // No answer to the question.
        Ok(serde_json::from_value(json!({"model": "jev-1.13.0", "answers": {}})).unwrap()),
        // A body the client cannot read.
        Err(BackendError::Unreadable("expected value".into())),
    ]);
    let ctx = w.ctx(fake.clone());
    for i in 0..6 {
        let out = decide(&ctx, request(Feature::WorkLink, Some(w.org))).await;
        assert_eq!(out.fallback, Some(Fallback::InvalidAnswer), "case {i}");
        assert!(out.answer.is_none(), "case {i}");
    }
    let runs = w.runs();
    assert_eq!(runs.len(), 6);
    assert!(runs.iter().all(|r| r.called
        && r.answer.is_none()
        && r.fallback.as_deref() == Some("invalid_answer")));
    // An invalid answer is an answer: the breaker stays closed.
    assert_eq!(w.gate(Feature::WorkLink, Some(w.org)), Ok(Mode::Shadow));
}

#[tokio::test]
async fn a_request_the_api_would_refuse_is_not_sent() {
    let w = world();
    w.all_on();
    let fake = Fake::answering(vec![]);
    let mut req = request(Feature::WorkLink, Some(w.org));
    req.request.question = Question::Choice {
        instructions: json!("pick"),
        criteria: BTreeMap::from([("only-one".to_string(), None)]),
    };
    let out = decide(&w.ctx(fake.clone()), req).await;
    assert_eq!(out.fallback, Some(Fallback::HttpError));
    assert_eq!(fake.calls(), 0);
    let mut big = request(Feature::WorkLink, Some(w.org));
    big.request.state = json!("x".repeat(jev::MAX_REQUEST_BYTES));
    let out = decide(&w.ctx(fake.clone()), big).await;
    assert_eq!(out.fallback, Some(Fallback::HttpError));
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().iter().all(|r| !r.called));
}

#[tokio::test(start_paused = true)]
async fn a_slow_answer_times_out() {
    let w = world();
    w.all_on();
    w.set(settings::DECIDE_JEV_TIMEOUT_MS, "200");
    let fake = Arc::new(Fake {
        script: Mutex::new(VecDeque::from([Ok(choice_response("ACME-1", 0.9, 10))])),
        delay: Some(Duration::from_secs(5)),
        ..Default::default()
    });
    let out = decide(
        &w.ctx(fake.clone()),
        request(Feature::WorkLink, Some(w.org)),
    )
    .await;
    assert_eq!(out.fallback, Some(Fallback::Timeout));
    let r = &w.runs()[0];
    assert!(r.called);
    assert_eq!(r.fallback.as_deref(), Some("timeout"));
    assert!(r.latency_ms.unwrap() >= 200);
}

#[tokio::test]
async fn the_breaker_opens_after_consecutive_failures_and_half_opens() {
    let w = world();
    w.all_on();
    w.set(settings::DECIDE_JEV_BREAKER_FAILURES, "2");
    w.set(settings::DECIDE_JEV_BREAKER_OPEN_SECS, "300");
    let fake = Fake::answering(vec![
        Err(BackendError::Http { status: 500 }),
        Err(BackendError::RateLimited {
            retry_after: Some(7),
        }),
        Err(BackendError::Overloaded),
        Ok(choice_response("ACME-1", 0.9, 10)),
    ]);
    let ctx = w.ctx(fake.clone());
    let ask = || decide(&ctx, request(Feature::WorkLink, Some(w.org)));
    assert_eq!(ask().await.fallback, Some(Fallback::HttpError));
    assert_eq!(ask().await.fallback, Some(Fallback::RateLimited));
    // Open: refused without a call.
    assert_eq!(ask().await.fallback, Some(Fallback::BreakerOpen));
    assert_eq!(fake.calls(), 2);
    let b = breaker_state(
        &w.store.lock().unwrap(),
        PROVIDER_JEV,
        2,
        300,
        NOON,
        RunScope::Live,
    )
    .unwrap();
    assert!(b.open);
    assert_eq!(b.open_until, Some(NOON + 300));
    // Half-open after the window: one call; a failure (529) opens it again.
    w.advance(301);
    assert_eq!(ask().await.fallback, Some(Fallback::RateLimited));
    assert_eq!(ask().await.fallback, Some(Fallback::BreakerOpen));
    assert_eq!(fake.calls(), 3);
    w.advance(301);
    let ok = ask().await;
    assert_eq!(ok.fallback, None);
    assert_eq!(ok.usable().unwrap().value, "ACME-1");
    // A success closes it.
    assert!(
        !breaker_state(
            &w.store.lock().unwrap(),
            PROVIDER_JEV,
            2,
            300,
            NOON + 602,
            RunScope::Live
        )
        .unwrap()
        .open
    );
}

#[tokio::test]
async fn the_daily_budget_stops_calls_until_the_next_utc_day() {
    let w = world();
    w.all_on();
    w.set(settings::DECIDE_JEV_DAILY_TOKEN_BUDGET, "1000");
    let fake = Fake::answering(vec![
        Ok(choice_response("ACME-1", 0.9, 1_200)),
        Ok(choice_response("ACME-1", 0.9, 10)),
    ]);
    let ctx = w.ctx(fake.clone());
    assert_eq!(
        decide(&ctx, request(Feature::WorkLink, Some(w.org)))
            .await
            .fallback,
        None
    );
    // The budget is shared by every feature.
    w.set(settings::DECIDE_JEV_STATUS_MAP, "shadow");
    let mut sm = request(Feature::StatusMap, Some(w.org));
    sm.subject_kind = "tracker".into();
    assert_eq!(decide(&ctx, sm).await.fallback, Some(Fallback::Budget));
    assert_eq!(fake.calls(), 1);
    w.advance(12 * 3600);
    assert_eq!(
        decide(&ctx, request(Feature::WorkLink, Some(w.org)))
            .await
            .fallback,
        None
    );
    // A zero budget allows nothing.
    w.set(settings::DECIDE_JEV_DAILY_TOKEN_BUDGET, "0");
    assert_eq!(
        w.gate(Feature::WorkLink, Some(w.org)),
        Err(Fallback::Budget)
    );
}

#[tokio::test]
async fn every_path_records_exactly_one_run() {
    let w = world();
    let fake = Fake::answering(vec![
        Err(BackendError::Timeout),
        Err(BackendError::Transport("connection refused".into())),
        Ok(choice_response("ACME-1", 0.9, 10)),
    ]);
    let ctx = w.ctx(fake.clone());
    let mut expected = Vec::new();
    let mut go = |out: DecisionOutcome| {
        assert!(out.run_id.is_some());
        expected.push(out.fallback.map(|f| f.as_str()).unwrap_or("answered"));
    };
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // flag_off
    w.set(settings::DECIDE_JEV_ENABLED, "true");
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // mode_off
    w.set(settings::DECIDE_JEV_WORK_LINK, "shadow");
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // org_off
    w.store
        .lock()
        .unwrap()
        .set_org_jev_allowed(w.org, true)
        .unwrap();
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // no_key
    w.store
        .lock()
        .unwrap()
        .set_decision_credential(Some(&Secret::new(KEY)), None)
        .unwrap();
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // timeout
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // http_error
    go(decide(&ctx, request(Feature::WorkLink, Some(w.org))).await); // answered
    assert_eq!(
        expected,
        vec![
            "flag_off",
            "mode_off",
            "org_off",
            "no_key",
            "timeout",
            "http_error",
            "answered"
        ]
    );
    let runs = w.runs();
    assert_eq!(runs.len(), expected.len());
    let recorded: Vec<&str> = runs
        .iter()
        .rev()
        .map(|r| r.fallback.as_deref().unwrap_or("answered"))
        .collect();
    assert_eq!(recorded, expected);
    // The configured mode is recorded even on a refusal.
    assert_eq!(runs.last().unwrap().mode, "off");
    assert_eq!(runs[0].mode, "shadow");
}

// --- redaction and the fingerprint ----------------------------------------------

#[test]
fn redact_state_removes_urls_emails_and_tokens() {
    let t = redact_state(
        "see https://acme.atlassian.net/browse/ACME-1?token=s3cr3t, mail bob.smith+x@acme.co.uk, \
         Authorization: Bearer ghp_0123456789abcdefghijklmnopqrstuvwxyz and ACME-7",
    );
    assert!(!t.contains("atlassian"), "{t}");
    assert!(!t.contains("s3cr3t"), "{t}");
    assert!(!t.contains("bob.smith"), "{t}");
    assert!(!t.contains("ghp_0123"), "{t}");
    assert!(t.contains("[url]") && t.contains("[email]"), "{t}");
    assert!(t.contains("ACME-7"), "ticket keys survive: {t}");
}

#[tokio::test]
async fn what_is_sent_is_redacted() {
    let w = world();
    w.all_on();
    let fake = Fake::answering(vec![Ok(choice_response("ACME-1", 0.9, 10))]);
    let mut req = request(Feature::WorkLink, Some(w.org));
    req.request.question = Question::Choice {
        instructions: json!({"task": "pick, cf. http://intranet/x"}),
        criteria: BTreeMap::from([
            ("ACME-1".to_string(), Some(json!("ask carol@acme.io"))),
            ("ACME-2".to_string(), None),
        ]),
    };
    decide(&w.ctx(fake.clone()), req).await;
    let (_, _, sent) = fake.seen.lock().unwrap()[0].clone();
    let body = serde_json::to_string(&sent).unwrap();
    for gone in ["atlassian", "abc123", "bob@acme.io", "intranet", "carol@"] {
        assert!(!body.contains(gone), "{gone} was sent: {body}");
    }
    assert!(body.contains("ACME-1"), "ids stay: {body}");
}

#[test]
fn the_fingerprint_is_a_keyed_hmac_not_a_plain_hash() {
    use sha2::Digest as _;
    let req = choice_request().redacted();
    let k1 = Secret::new("a".repeat(64));
    let k2 = Secret::new("b".repeat(64));
    let fp = fingerprint(&k1, &req);
    assert_eq!(fp.len(), 64);
    assert_eq!(fp, fingerprint(&k1, &req), "deterministic");
    assert_ne!(fp, fingerprint(&k2, &req), "keyed");
    let canonical = canonical_json(&json!({"state": req.state, "question": req.question}));
    let plain = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
    assert_ne!(fp, plain, "not a plain SHA-256 of the input");
    // Canonical: key order does not change it.
    assert_eq!(
        canonical_json(&json!({"b": 1, "a": {"d": 2, "c": 3}})),
        r#"{"a":{"c":3,"d":2},"b":1}"#
    );
}

// --- the key never shows ----------------------------------------------------------

#[tokio::test]
async fn the_key_is_never_in_a_run_an_outcome_a_status_or_a_debug_line() {
    let w = world();
    w.all_on();
    let fake = Fake::answering(vec![Err(BackendError::Transport(format!(
        "echoing Bearer {KEY}"
    )))]);
    let out = decide(&w.ctx(fake), request(Feature::WorkLink, Some(w.org))).await;
    assert!(!format!("{out:?}").contains(KEY));
    assert!(!serde_json::to_string(&w.runs()).unwrap().contains(KEY));
    let st = status(&w.store.lock().unwrap(), NOON, 30).unwrap();
    assert!(!serde_json::to_string(&st).unwrap().contains(KEY));
    assert!(!st.lines().join("\n").contains(KEY));
    assert!(st.key.configured);
    // The request a transport would log masks the Authorization header.
    let t = FakeTransport::new();
    t.once(
        Method::Post,
        "/v1/systemone",
        Ok(Response::json(
            200,
            &json!({"model": "jev-1.13.0", "answers": {}}),
        )),
    );
    JevBackend::new(Arc::new(t.clone()))
        .ask(
            &Secret::new(KEY),
            "jev-1.13.0",
            &choice_request(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    let sent = &t.requests()[0];
    assert_eq!(
        sent.header_value("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert!(!format!("{sent:?}").contains(KEY));
    // Diagnostics mask it.
    let lits = w.store.lock().unwrap().decision_secret_literals().unwrap();
    assert!(lits.contains(&KEY.to_string()));
}

// --- the Jev client -----------------------------------------------------------------

#[tokio::test]
async fn the_jev_client_speaks_the_api_and_maps_its_errors() {
    let t = FakeTransport::new();
    let ok = json!({
        "model": "jev-1.13.0",
        "answers": {"q": {"type": "choice", "choice": "ACME-1",
                          "probabilities": {"ACME-1": 0.7, "ACME-2": 0.3}, "confidence": 0.81}},
        "usage": {"input_tokens": 321, "output_tokens": 4}
    });
    t.once(Method::Post, "/v1/systemone", Ok(Response::json(200, &ok)));
    t.once(
        Method::Post,
        "/v1/systemone",
        Ok(Response::new(429, "slow down").with_header("Retry-After", "7")),
    );
    t.once(Method::Post, "/v1/systemone", Ok(Response::new(529, "")));
    t.once(Method::Post, "/v1/systemone", Ok(Response::new(401, "")));
    t.once(Method::Post, "/v1/systemone", Ok(Response::new(422, "")));
    t.once(
        Method::Post,
        "/v1/systemone",
        Ok(Response::new(200, "<html>")),
    );
    t.once(
        Method::Post,
        "/v1/systemone",
        Err(crate::net::https::TransportError::Timeout),
    );
    let b = JevBackend::new(Arc::new(t.clone()));
    let (key, req) = (Secret::new(KEY), choice_request());
    let ask = || b.ask(&key, "jev-1.13.0", &req, Duration::from_millis(1500));
    let r = ask().await.unwrap();
    assert_eq!(r.usage.input_tokens, 321);
    assert!(matches!(r.answers["q"], Answer::Choice { .. }));
    assert_eq!(
        ask().await.unwrap_err(),
        BackendError::RateLimited {
            retry_after: Some(7)
        }
    );
    assert_eq!(ask().await.unwrap_err(), BackendError::Overloaded);
    assert_eq!(ask().await.unwrap_err(), BackendError::Http { status: 401 });
    assert_eq!(ask().await.unwrap_err(), BackendError::Http { status: 422 });
    assert!(matches!(
        ask().await.unwrap_err(),
        BackendError::Unreadable(_)
    ));
    assert_eq!(ask().await.unwrap_err(), BackendError::Timeout);
    let sent = &t.requests()[0];
    assert_eq!(sent.url, JEV_URL);
    assert_eq!(sent.timeout, Duration::from_millis(1500));
    let body = sent.json_body().unwrap();
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["questions"]["q"]["type"], "choice");
    assert_eq!(
        body["questions"]["q"]["criteria"]["ACME-2"],
        "Billing export"
    );
    assert!(body["state"]["prompt"].is_string());
    // The mapping to fallbacks.
    assert_eq!(BackendError::Overloaded.fallback(), Fallback::RateLimited);
    assert_eq!(
        BackendError::Http { status: 401 }.fallback(),
        Fallback::HttpError
    );
    assert_eq!(
        BackendError::Unreadable(String::new()).fallback(),
        Fallback::InvalidAnswer
    );
}

#[test]
fn the_transport_is_fenced_to_the_api_host() {
    let p = jev::host_policy();
    assert!(p("api.typesafe.ai"));
    assert!(p("API.TypeSafe.ai"));
    for bad in [
        "typesafe.ai",
        "evil.api.typesafe.ai",
        "api.typesafe.ai.evil.com",
        "169.254.169.254",
        "localhost",
    ] {
        assert!(!p(bad), "{bad}");
    }
}

#[test]
fn answers_are_checked_against_their_question() {
    use jev::validate_answer as v;
    let noul = Question::Noul {
        instructions: json!("is it?"),
        criteria: None,
    };
    let a = v(&noul, &Answer::Noul { noul: 0.25 }).unwrap();
    assert_eq!(a.value, "0.25");
    assert_eq!(a.confidence, Some(0.75));
    assert!(v(&noul, &Answer::Noul { noul: 1.5 }).is_err());
    assert!(v(&noul, &Answer::Noul { noul: f64::NAN }).is_err());
    let score = Question::Score {
        instructions: json!("how done?"),
        criteria: vec![json!("not"), json!("half"), json!("done")],
    };
    let ok = Answer::Score {
        score: 1.05,
        legend: BTreeMap::new(),
        probabilities: BTreeMap::from([("0".into(), 0.1), ("1".into(), 0.75), ("2".into(), 0.15)]),
        confidence: Some(0.7),
    };
    assert_eq!(v(&score, &ok).unwrap().value, "1.05");
    let over = Answer::Score {
        score: 2.5,
        legend: BTreeMap::new(),
        probabilities: BTreeMap::new(),
        confidence: None,
    };
    assert!(v(&score, &over).is_err());
    assert!(score.check().is_ok());
    assert!(Question::Score {
        instructions: json!(""),
        criteria: vec![json!("one")]
    }
    .check()
    .is_err());
    // A choice's options must be ids or words: they are recorded.
    assert!(Question::Choice {
        instructions: json!(""),
        criteria: BTreeMap::from([("a sentence with spaces".into(), None), ("ok".into(), None)]),
    }
    .check()
    .is_err());
    assert_eq!(jev::cost_microusd(0), 0);
    assert_eq!(jev::cost_microusd(1), 1);
    assert_eq!(jev::cost_microusd(1_000_000), 42_000);
    assert_eq!(jev::fmt_num(1.0), "1");
    assert_eq!(jev::fmt_num(0.0), "0");
}

// --- vocabulary, settings, retention ----------------------------------------------------

#[test]
fn the_fallback_vocabulary_is_the_stores() {
    let ours: Vec<&str> = Fallback::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(ours, crate::store::DECISION_FALLBACKS);
    for f in Fallback::ALL {
        assert_eq!(Fallback::parse(f.as_str()), Some(f));
        assert_eq!(serde_json::to_value(f).unwrap(), json!(f.as_str()));
    }
    let migration = include_str!("../../../migrations/069_decision_runs.sql");
    for f in crate::store::DECISION_FALLBACKS {
        assert!(
            migration.contains(&format!("'{f}'")),
            "{f} not in the CHECK"
        );
    }
    for f in Feature::ALL {
        assert_eq!(Feature::parse(f.as_str()), Some(f));
        let spec = settings::spec(f.setting_key()).unwrap();
        assert_eq!(spec.default, "off");
        assert_eq!(f.setting_key(), format!("decide.jev.{}", f.as_str()));
    }
}

#[test]
fn the_retention_sweep_follows_its_setting() {
    let w = world();
    let old = crate::store::NewDecisionRun {
        at: NOON - 91 * 86_400,
        feature: "work_link".into(),
        subject_kind: "session".into(),
        subject_id: "1".into(),
        mode: "off".into(),
        provider: "jev".into(),
        question_version: "wl.1".into(),
        fallback: Some("flag_off".into()),
        ..Default::default()
    };
    let recent = crate::store::NewDecisionRun {
        at: NOON - 89 * 86_400,
        ..old.clone()
    };
    {
        let s = w.store.lock().unwrap();
        s.insert_decision_run(&old).unwrap();
        s.insert_decision_run(&recent).unwrap();
    }
    w.set(settings::DECIDE_RETENTION_DAYS, "0");
    assert_eq!(sweep_runs(&w.store, NOON), 0, "0 keeps forever");
    w.set(settings::DECIDE_RETENTION_DAYS, "90");
    assert_eq!(sweep_runs(&w.store, NOON), 1);
    assert_eq!(w.runs().len(), 1);
    assert_eq!(sweep_runs(&w.store, NOON), 0);
}

#[test]
fn status_reads_without_writing() {
    let w = world();
    w.all_on();
    let st = status(&w.store.lock().unwrap(), NOON, 7).unwrap();
    assert!(st.enabled && st.owns_the_fleet);
    assert_eq!(st.modes["work_link"], "shadow");
    assert_eq!(st.modes["status_map"], "off");
    assert_eq!(st.orgs_allowed.len(), 1);
    assert_eq!(st.today.budget, 2_000_000);
    assert_eq!(st.today.since, NOON - 12 * 3600);
    assert!(!st.breaker.open);
    let lines = st.lines().join("\n");
    assert!(lines.contains("decisions (Jev): on"), "{lines}");
    assert!(lines.contains("no runs in the last 7 days"), "{lines}");
    assert_eq!(fmt_usd(51), "$0.000051");
}

// --- the offline benchmark's calls ---------------------------------------------------

fn bench_request(feature: Feature, org_id: Option<i64>) -> DecideRequest {
    DecideRequest {
        subject_kind: DECISION_BENCH_SUBJECT.into(),
        subject_id: "s1".into(),
        ..request(feature, org_id)
    }
}

#[test]
fn the_benchmark_needs_no_live_mode_but_every_other_check() {
    let w = world();
    let bench = |f: Feature, org: Option<i64>| {
        gate_bench_at(
            &w.store.lock().unwrap(),
            f,
            org,
            w.clock.load(Ordering::SeqCst),
        )
    };
    let f = Feature::StatusMap;
    assert_eq!(bench(f, Some(w.org)), Err(Fallback::FlagOff));
    w.set(settings::DECIDE_JEV_ENABLED, "true");
    // The live mode stays off: the live gate refuses, the benchmark's does
    // not stop there.
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::ModeOff));
    assert_eq!(bench(f, Some(w.org)), Err(Fallback::OrgOff));
    w.store
        .lock()
        .unwrap()
        .set_org_jev_allowed(w.org, true)
        .unwrap();
    assert_eq!(bench(f, Some(w.org)), Err(Fallback::NoKey));
    w.store
        .lock()
        .unwrap()
        .set_decision_credential(Some(&Secret::new(KEY)), None)
        .unwrap();
    // Always shadow: a benchmark never proposes, even with assist on.
    assert_eq!(bench(f, Some(w.org)), Ok(Mode::Shadow));
    w.set(settings::DECIDE_JEV_STATUS_MAP, "assist");
    assert_eq!(bench(f, Some(w.org)), Ok(Mode::Shadow));
    w.set(settings::DECIDE_JEV_STATUS_MAP, "off");
    assert_eq!(w.gate(f, Some(w.org)), Err(Fallback::ModeOff));
    // A window onto a hub never calls out, benchmark or not.
    w.store
        .lock()
        .unwrap()
        .set_setting(HUB_REMOTE_URL_KEY, "https://hub.example.com")
        .unwrap();
    assert_eq!(bench(f, Some(w.org)), Err(Fallback::NotOwner));
}

#[tokio::test]
async fn a_benchmarks_spend_and_failures_never_stop_the_live_calls() {
    let w = world();
    w.all_on();
    w.set(settings::DECIDE_JEV_DAILY_TOKEN_BUDGET, "1000");
    w.set(settings::DECIDE_JEV_BREAKER_FAILURES, "2");
    let fake = Fake::answering(vec![
        Ok(choice_response("ACME-1", 0.9, 1_200)),
        Err(BackendError::Http { status: 500 }),
        Err(BackendError::Http { status: 500 }),
        Ok(choice_response("ACME-1", 0.9, 10)),
    ]);
    let ctx = w.ctx(fake.clone());
    // The benchmark spends the day's budget, then fails twice...
    let spent = decide(&ctx, bench_request(Feature::WorkLink, Some(w.org))).await;
    assert_eq!((spent.fallback, spent.mode), (None, Some(Mode::Shadow)));
    // ...its own calls see the spend (over every run): refused.
    let refused = decide(&ctx, bench_request(Feature::WorkLink, Some(w.org))).await;
    assert_eq!(refused.fallback, Some(Fallback::Budget));
    w.set(settings::DECIDE_JEV_DAILY_TOKEN_BUDGET, "1000000");
    for _ in 0..2 {
        let r = decide(&ctx, bench_request(Feature::WorkLink, Some(w.org))).await;
        assert_eq!(r.fallback, Some(Fallback::HttpError));
    }
    assert_eq!(
        decide(&ctx, bench_request(Feature::WorkLink, Some(w.org)))
            .await
            .fallback,
        Some(Fallback::BreakerOpen)
    );
    w.set(settings::DECIDE_JEV_DAILY_TOKEN_BUDGET, "1000");
    // The live path: its breaker is closed and its budget unspent.
    let live = decide(&ctx, request(Feature::WorkLink, Some(w.org))).await;
    assert_eq!(live.fallback, None);
    assert_eq!(fake.calls(), 4);
    // Every benchmark run records shadow, whatever the live mode.
    let runs = w.runs();
    assert!(runs
        .iter()
        .filter(|r| r.subject_kind == DECISION_BENCH_SUBJECT)
        .all(|r| r.mode == "shadow"));
    // `decide status` keeps them apart.
    let st = status(&w.store.lock().unwrap(), NOON, 1).unwrap();
    assert_eq!(st.today.input_tokens, 10);
    assert_eq!(st.today.bench_input_tokens, 1_200);
    assert!(!st.breaker.open);
    let live_rows: i64 = st.stats.iter().filter(|r| !r.bench).map(|r| r.runs).sum();
    assert_eq!(live_rows, 1);
    assert!(st.lines().iter().any(|l| l.contains("bench")));
}

// --- health (test map §7) -------------------------------------------------------------

fn live_run(fallback: Option<&str>, at: i64) -> crate::store::NewDecisionRun {
    crate::store::NewDecisionRun {
        at,
        feature: "status_map".into(),
        subject_kind: "tracker_section".into(),
        subject_id: "1:abc".into(),
        mode: "shadow".into(),
        provider: "jev".into(),
        question_version: "status_map.v1".into(),
        fallback: fallback.map(str::to_string),
        called: fallback.is_none_or(|f| {
            matches!(
                f,
                "timeout" | "http_error" | "rate_limited" | "invalid_answer" | "low_confidence"
            )
        }),
        ..Default::default()
    }
}

#[test]
fn health_is_absent_while_nothing_is_on_and_ok_on_a_quiet_hour() {
    let w = world();
    assert_eq!(health(&w.store.lock().unwrap(), NOON), None);
    w.all_on();
    let h = health(&w.store.lock().unwrap(), NOON).unwrap();
    assert!(h.enabled && !h.degraded);
    assert_eq!(h.modes.keys().collect::<Vec<_>>(), vec!["work_link"]);
    assert_eq!((h.attempts, h.failures, h.failure_rate), (0, 0, None));
    assert!(h.line().ends_with("→ ok"), "{}", h.line());
    // A window onto a hub reports nothing: the hub does.
    w.store
        .lock()
        .unwrap()
        .set_setting(HUB_REMOTE_URL_KEY, "https://hub.example")
        .unwrap();
    assert_eq!(health(&w.store.lock().unwrap(), NOON), None);
}

#[test]
fn failing_calls_in_the_last_hour_make_it_degraded() {
    let w = world();
    w.all_on();
    {
        let s = w.store.lock().unwrap();
        // Refusals by configuration are no attempts; an old failure is out
        // of the window; a benchmark's failures never count.
        for _ in 0..20 {
            s.insert_decision_run(&live_run(Some("org_off"), NOON - 60))
                .unwrap();
        }
        for _ in 0..10 {
            s.insert_decision_run(&live_run(Some("timeout"), NOON - 2 * 3600))
                .unwrap();
            s.insert_decision_run(&crate::store::NewDecisionRun {
                subject_kind: DECISION_BENCH_SUBJECT.into(),
                ..live_run(Some("http_error"), NOON - 60)
            })
            .unwrap();
        }
        for _ in 0..8 {
            s.insert_decision_run(&live_run(None, NOON - 120)).unwrap();
        }
        s.insert_decision_run(&live_run(Some("low_confidence"), NOON - 120))
            .unwrap();
        s.insert_decision_run(&live_run(Some("timeout"), NOON - 60))
            .unwrap();
    }
    // 1 failure of 10 attempts: 10%, ok.
    let h = health(&w.store.lock().unwrap(), NOON).unwrap();
    assert_eq!((h.attempts, h.failures), (10, 1));
    assert_eq!(h.failure_rate, Some(0.1));
    assert!(!h.degraded);
    {
        let s = w.store.lock().unwrap();
        s.insert_decision_run(&live_run(Some("rate_limited"), NOON - 30))
            .unwrap();
        s.insert_decision_run(&live_run(Some("http_error"), NOON - 20))
            .unwrap();
    }
    // 3 of 12: 25% > 20%.
    let h = health(&w.store.lock().unwrap(), NOON).unwrap();
    assert_eq!((h.attempts, h.failures), (12, 3));
    assert!(h.degraded);
    assert_eq!(h.reason.as_deref(), Some("failure_rate"));
    assert!(h.line().contains("DEGRADED (failure_rate)"), "{}", h.line());
    // The kill switch off: nothing is on, whatever the record says.
    w.set(settings::DECIDE_JEV_ENABLED, "false");
    let h = health(&w.store.lock().unwrap(), NOON).unwrap();
    assert!(
        !h.degraded,
        "a mode left on with the switch off is not degraded"
    );
}

#[test]
fn an_open_breaker_is_degraded_with_few_attempts_and_a_spent_budget_is_not() {
    let mut modes = BTreeMap::new();
    modes.insert("status_map".to_string(), "assist".to_string());
    let h = health_of(true, modes.clone(), &[], true, false);
    assert_eq!(
        (h.degraded, h.reason.as_deref()),
        (true, Some("breaker_open"))
    );
    let row = |fallback: Option<&str>, runs: i64| DecisionStatRow {
        feature: "status_map".into(),
        provider: "jev".into(),
        fallback: fallback.map(str::to_string),
        runs,
        ..Default::default()
    };
    // Under five attempts a failure rate says nothing.
    let h = health_of(
        true,
        modes.clone(),
        &[row(Some("timeout"), 4)],
        false,
        false,
    );
    assert_eq!(h.failure_rate, Some(1.0));
    assert!(!h.degraded);
    // A spent budget is planned: reported, not degraded, and no attempt.
    let h = health_of(
        true,
        modes,
        &[row(Some("budget"), 50), row(None, 5)],
        false,
        true,
    );
    assert!(h.budget_spent && !h.degraded);
    assert_eq!(h.attempts, 5);
    assert!(h.line().contains("today's budget spent"), "{}", h.line());
}

#[test]
fn status_shows_the_health_line() {
    let w = world();
    w.all_on();
    let st = status(&w.store.lock().unwrap(), NOON, 7).unwrap();
    assert!(st.health.is_some());
    assert!(st.lines().join("\n").contains("health (last 60 min, live)"));
}
