//! The second opinion's tests: off by default, its fences, the run it
//! records beside Jev's, and that the proposal readers never see it. No
//! test reaches a host: [`ScriptedSsh`] answers.

use super::haiku::testing::ScriptedSsh;
use super::haiku::{reason, HaikuReply, PROVIDER_HAIKU};
use super::haiku_shadow::{plan, Job};
use super::*;
use crate::store::{DecisionRunFilter, NewDecisionRun};
use serde_json::json;

/// Noon, UTC.
const NOON: i64 = 1_790_510_400;

fn request(feature: Feature, org_id: Option<i64>) -> DecideRequest {
    DecideRequest {
        feature,
        subject_kind: "session".into(),
        subject_id: "42".into(),
        org_id,
        request: JevRequest {
            state: json!({ "prompt": "fix the login redirect, mail bob@acme.io" }),
            question: Question::Choice {
                instructions: json!("Which ticket is this session working on?"),
                criteria: BTreeMap::from([
                    ("ACME-1".to_string(), Some(json!("Login redirect loops"))),
                    ("ACME-2".to_string(), Some(json!("Billing export"))),
                ]),
            },
        },
        baseline: Some("ACME-2".into()),
        question_version: "wl.1".into(),
        min_confidence: Some(0.6),
    }
}

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
    host: &'static str,
    ssh: Arc<ScriptedSsh>,
}

/// A store whose host `host` (each test its own, as the in-flight slots
/// are per process) is in `Acme`, the second opinion on and pointed at
/// it, `work_link` in shadow; the host answers `answer`.
fn world(host: &'static str, answer: &'static str) -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.upsert_host(host).unwrap();
    s.set_host_org(host, Some(org)).unwrap();
    for (k, v) in [
        (settings::DECIDE_HAIKU_SHADOW, "true"),
        (settings::DECIDE_HAIKU_HOST, host),
        (settings::DECIDE_JEV_WORK_LINK, "shadow"),
    ] {
        settings::set(&s, k, v).unwrap();
    }
    World {
        store: Arc::new(Mutex::new(s)),
        org,
        host,
        ssh: Arc::new(ScriptedSsh::new(move |_| answer.to_string())),
    }
}

impl World {
    fn ctx(&self) -> DecideCtx {
        DecideCtx::jev(Arc::clone(&self.store))
            .with_clock(Arc::new(|| NOON))
            .with_haiku_exec(Arc::clone(&self.ssh) as Arc<dyn crate::ssh::SshExec>)
    }
    fn set(&self, key: &str, value: &str) {
        settings::set(&self.store.lock().unwrap(), key, value).unwrap();
    }
    fn plan(&self, req: &DecideRequest) -> Option<Job> {
        plan(&self.ctx(), req)
    }
    fn runs(&self, provider: &str) -> Vec<crate::store::DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter {
                provider: Some(provider.into()),
                ..Default::default()
            })
            .unwrap()
    }
}

const ACME_1: &str = r#"{"choice":"ACME-1","confidence":0.8}"#;

#[test]
fn with_the_defaults_nothing_is_planned() {
    let s = Store::open_in_memory().unwrap();
    assert!(!settings::get_bool(&s, settings::DECIDE_HAIKU_SHADOW));
    assert_eq!(settings::get_string(&s, settings::DECIDE_HAIKU_HOST), "");
    assert_eq!(
        settings::get_string(&s, settings::DECIDE_HAIKU_MODEL),
        "haiku"
    );
    assert_eq!(
        settings::get_string(&s, settings::DECIDE_HAIKU_DAILY_CALLS),
        "200"
    );
    let w = World {
        store: Arc::new(Mutex::new(s)),
        org: 0,
        host: "unused",
        ssh: Arc::new(ScriptedSsh::new(|_| ACME_1.into())),
    };
    for &f in Feature::ALL {
        assert!(w.plan(&request(f, None)).is_none(), "{f:?}");
    }
}

#[test]
fn each_fence_keeps_the_question_out() {
    let w = world("hs-fences", ACME_1);
    let req = request(Feature::WorkLink, Some(w.org));
    assert!(w.plan(&req).is_some(), "the world is cleared");

    // Another org's question never goes to this host, nor one with none.
    assert!(w.plan(&request(Feature::WorkLink, None)).is_none());
    assert!(w
        .plan(&request(Feature::WorkLink, Some(w.org + 1)))
        .is_none());
    // A benchmark's call is the benchmark's to make.
    let mut bench = req.clone();
    bench.subject_kind = DECISION_BENCH_SUBJECT.into();
    assert!(w.plan(&bench).is_none());
    // Only a Choice.
    let mut noul = req.clone();
    noul.request.question = Question::Noul {
        instructions: json!("How sure?"),
        criteria: None,
    };
    assert!(w.plan(&noul).is_none());
    // Never Claude's reply text, even with the use case on.
    w.set(settings::DECIDE_JEV_TURN_OUTCOME, "shadow");
    assert!(w
        .plan(&request(Feature::TurnOutcome, Some(w.org)))
        .is_none());
    // A use case that is off is never asked about.
    assert!(w.plan(&request(Feature::StatusMap, Some(w.org))).is_none());
    // A window onto a hub never calls out.
    let remote = |v: &str| {
        w.store
            .lock()
            .unwrap()
            .set_setting(HUB_REMOTE_URL_KEY, v)
            .unwrap()
    };
    remote("https://hub.example");
    assert!(w.plan(&req).is_none());
    remote("");
    // No host, or one this database does not know.
    w.set(settings::DECIDE_HAIKU_HOST, "");
    assert!(w.plan(&req).is_none());
    w.set(settings::DECIDE_HAIKU_HOST, "stranger");
    assert!(w.plan(&req).is_none());
    w.set(settings::DECIDE_HAIKU_HOST, w.host);
    // The switch.
    w.set(settings::DECIDE_HAIKU_SHADOW, "false");
    assert!(w.plan(&req).is_none());
    assert_eq!(w.ssh.calls(), 0);
}

#[test]
fn one_question_at_a_time_per_host() {
    let w = world("hs-slot", ACME_1);
    let req = request(Feature::WorkLink, Some(w.org));
    let first = w.plan(&req).expect("cleared");
    assert!(w.plan(&req).is_none(), "the host is busy");
    drop(first);
    assert!(w.plan(&req).is_some(), "the slot is freed with the job");
}

#[tokio::test]
async fn the_answer_is_recorded_beside_jevs() {
    let w = world("hs-record", ACME_1);
    let out = decide(&w.ctx(), request(Feature::WorkLink, Some(w.org))).await;
    // Jev's own path is as it was: its flag is off.
    assert_eq!(out.fallback, Some(Fallback::FlagOff));
    let mut haiku = Vec::new();
    for _ in 0..200 {
        haiku = w.runs(PROVIDER_HAIKU);
        if !haiku.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(haiku.len(), 1, "the second opinion was recorded");
    let jev = w.runs(PROVIDER_JEV);
    assert_eq!(jev.len(), 1);
    let (h, j) = (&haiku[0], &jev[0]);
    assert_eq!(h.mode, "shadow");
    assert_eq!(h.model_version.as_deref(), Some("haiku"));
    assert_eq!(h.answer.as_deref(), Some("ACME-1"));
    assert_eq!(h.confidence, Some(0.8));
    assert_eq!(h.fallback, None);
    assert!(h.called);
    assert_eq!(h.input_tokens, 100, "plain and cached");
    assert_eq!(h.cost_microusd, 500);
    assert_eq!(
        (&h.feature, &h.subject_kind, &h.subject_id, h.org_id),
        (&j.feature, &j.subject_kind, &j.subject_id, j.org_id)
    );
    assert_eq!(h.question_version, j.question_version);
    assert_eq!(h.candidates, j.candidates);
    assert_eq!(h.baseline_answer, j.baseline_answer);
    assert!(h.input_fp.is_some());
    assert_eq!(h.input_fp, j.input_fp, "the same redacted question");
    // The redacted request went to the configured host, and nothing else.
    assert_eq!(*w.ssh.hosts.lock().unwrap(), vec![w.host.to_string()]);
    let prompt = w.ssh.prompts.lock().unwrap()[0].clone();
    assert!(!prompt.contains("bob@acme.io"), "{prompt}");
}

#[tokio::test]
async fn the_daily_cap_counts_the_calls() {
    let w = world("hs-cap", ACME_1);
    w.set(settings::DECIDE_HAIKU_DAILY_CALLS, "1");
    let req = request(Feature::WorkLink, Some(w.org));
    let id = w.plan(&req).expect("cleared").run().await;
    assert!(id.is_some());
    assert!(w.plan(&req).is_none(), "the day's one call is spent");
    w.set(settings::DECIDE_HAIKU_DAILY_CALLS, "2");
    assert!(w.plan(&req).is_some());
    w.set(settings::DECIDE_HAIKU_DAILY_CALLS, "0");
    assert!(w.plan(&req).is_none(), "0 is none");
}

#[tokio::test]
async fn a_low_or_invalid_answer_is_a_fallback() {
    let w = world("hs-low", r#"{"choice":"ACME-1","confidence":0.3}"#);
    let req = request(Feature::WorkLink, Some(w.org));
    w.plan(&req).unwrap().run().await.unwrap();
    let r = &w.runs(PROVIDER_HAIKU)[0];
    assert_eq!(r.fallback.as_deref(), Some("low_confidence"));
    assert_eq!(r.answer.as_deref(), Some("ACME-1"));

    let w = world("hs-invalid", r#"{"choice":"ACME-9"}"#);
    let req = request(Feature::WorkLink, Some(w.org));
    w.plan(&req).unwrap().run().await.unwrap();
    let r = &w.runs(PROVIDER_HAIKU)[0];
    assert_eq!(r.fallback.as_deref(), Some("invalid_answer"));
    assert_eq!(r.answer, None);
}

#[test]
fn a_failed_call_maps_to_the_envelopes_fallbacks() {
    let fill = |reply: HaikuReply| {
        let mut run = NewDecisionRun {
            at: NOON,
            feature: "work_link".into(),
            org_id: None,
            subject_kind: "session".into(),
            subject_id: "42".into(),
            mode: "shadow".into(),
            provider: PROVIDER_HAIKU.into(),
            model_version: None,
            question_version: "wl.1".into(),
            input_fp: None,
            candidates: Vec::new(),
            answer: None,
            probabilities: None,
            confidence: None,
            fallback: None,
            baseline_answer: None,
            called: false,
            latency_ms: None,
            input_tokens: 0,
            cost_microusd: 0,
        };
        super::haiku_shadow::fill(&mut run, &reply, None);
        (run.fallback, run.called)
    };
    let failed = |error| HaikuReply {
        ran: true,
        error: Some(error),
        ..Default::default()
    };
    assert_eq!(
        fill(failed(reason::TIMEOUT)),
        (Some("timeout".into()), true)
    );
    for e in [reason::SSH_ERROR, reason::SIGNED_OUT, reason::NO_CLAUDE] {
        assert_eq!(fill(failed(e)), (Some("http_error".into()), true), "{e}");
    }
    let unsent = HaikuReply {
        error: Some(reason::TOO_LONG),
        ..Default::default()
    };
    assert_eq!(fill(unsent), (Some("http_error".into()), false));
}

#[test]
fn a_second_opinion_never_withdraws_jevs_proposal() {
    let w = world("hs-proposal", ACME_1);
    let s = w.store.lock().unwrap();
    let run = |provider: &str, mode: &str, answer: &str| NewDecisionRun {
        at: NOON,
        feature: "work_link".into(),
        org_id: Some(w.org),
        subject_kind: "session".into(),
        subject_id: "42".into(),
        mode: mode.into(),
        provider: provider.into(),
        model_version: None,
        question_version: "wl.1".into(),
        input_fp: None,
        candidates: vec!["ACME-1".into(), "ACME-2".into()],
        answer: Some(answer.into()),
        probabilities: None,
        confidence: Some(0.9),
        fallback: None,
        baseline_answer: None,
        called: true,
        latency_ms: Some(5),
        input_tokens: 1,
        cost_microusd: 1,
    };
    let jev = s
        .insert_decision_run(&run(PROVIDER_JEV, "assist", "ACME-2"))
        .unwrap();
    s.insert_decision_run(&run(PROVIDER_HAIKU, "shadow", "ACME-1"))
        .unwrap();
    let proposals = s.current_proposals("session").unwrap();
    let p = &proposals["42"];
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].run_id, p[0].value.as_str()), (Some(jev), "ACME-2"));
    // The adapters' own lookups see Jev's run only.
    let mine = s
        .decision_runs_for_subjects("work_link", "session", "42", None)
        .unwrap();
    assert_eq!(mine.iter().map(|r| r.id).collect::<Vec<_>>(), vec![jev]);
    // A newer Jev answer supersedes Jev's, and leaves the haiku run alone.
    let newer = s
        .insert_decision_run(&run(PROVIDER_JEV, "shadow", "ACME-1"))
        .unwrap();
    s.supersede_decision_runs("work_link", "session", "42", "shadow", newer, NOON)
        .unwrap();
    let haiku = s
        .list_decision_runs(&DecisionRunFilter {
            provider: Some(PROVIDER_HAIKU.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(haiku[0].followup, None);
}

#[test]
fn the_envelopes_health_skips_the_second_opinion() {
    let row = |provider: &str| DecisionStatRow {
        feature: "work_link".into(),
        bench: false,
        provider: provider.into(),
        fallback: Some("timeout".into()),
        org_id: None,
        runs: 9,
        ..Default::default()
    };
    let modes = BTreeMap::from([("work_link".to_string(), "shadow".to_string())]);
    let h = health_of(true, modes.clone(), &[row(PROVIDER_HAIKU)], false, false);
    assert_eq!((h.attempts, h.failures, h.degraded), (0, 0, false));
    let h = health_of(true, modes, &[row(PROVIDER_JEV)], false, false);
    assert_eq!((h.attempts, h.failures, h.degraded), (9, 9, true));
}
