//! K2 `control_route` over a scripted backend: the gate, the short-message
//! rule, assist and shadow, the targets offered, follow-ups, and no message
//! text in the record. No test reaches TypeSafe.

use super::control_route::*;
use super::*;
use crate::service::view_scope::ViewScope;
use crate::store::{DecisionRunFilter, DecisionRunRow, NewMission};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
/// Noon, UTC.
const NOON: i64 = 1_790_510_400;
const MESSAGE: &str = "How far did the federation handshake get last night?";

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
    let other = if choice == UNSURE { CONTROL } else { UNSURE };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 90, "output_tokens": 2 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    mission: i64,
    session: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("hosta").unwrap();
    let pid = s.upsert_project("acme", "fleet", "/p").unwrap();
    let session = s
        .upsert_session("fed-v2", "hosta", Some(pid), None, 0, 100, "running", None)
        .unwrap();
    s.upsert_session("old", "hosta", None, None, 0, 50, "killed", None)
        .unwrap();
    let m = s
        .create_mission(
            &NewMission {
                org_id: None,
                owner_person_id: None,
                root_item_id: None,
                name: "Hub federation v2",
                goal: "Two hubs exchange sessions over a signed handshake.",
                non_goals: None,
                done_when: &[],
                mode: None,
                level: None,
            },
            "test",
        )
        .unwrap();
    s.set_mission_state(m.id, None, "active", "test").unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        mission: m.id,
        session,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOON))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_CONTROL_ROUTE, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn m(&self) -> String {
        format!("m{}", self.mission)
    }
    fn s(&self) -> String {
        format!("s{}", self.session)
    }
}

fn scope() -> ViewScope {
    ViewScope::internal()
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("m1", 0.9)]);
    let r = propose(&w.ctx(&fake), &scope(), MESSAGE).await;
    assert_eq!(r.outcome, "none");
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn the_targets_are_active_missions_then_running_sessions() {
    let w = world();
    let s = w.store.lock().unwrap();
    let t = targets(&s, &scope()).unwrap();
    let words: Vec<String> = t.iter().map(Target::option).collect();
    assert_eq!(words, vec![w.m(), w.s()]);
    assert_eq!(t[0].name, "Hub federation v2");
    assert_eq!(t[1].detail, "acme/fleet");
}

#[tokio::test]
async fn assist_proposes_the_mission_the_message_is_about() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.m(), 0.86)]);
    let r = propose(&w.ctx(&fake), &scope(), MESSAGE).await;
    assert_eq!(r.outcome, "proposed");
    assert_eq!(r.target.as_deref(), Some(w.m().as_str()));
    let p = r.proposal.unwrap();
    assert_eq!(
        (p.feature.as_str(), p.confidence_pct),
        ("control_route", Some(86))
    );
    assert_eq!(r.targets.len(), 2);
    // The message reached the model; the record holds an HMAC, not the text.
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen[0].state["message"], MESSAGE);
    let runs = w.runs();
    assert!(runs[0].subject_id.starts_with("msg:"));
    assert!(!runs[0].subject_id.contains("federation"));
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(CONTROL));
}

#[tokio::test]
async fn a_short_message_is_asked_about_without_a_call() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("m1", 0.9)]);
    let r = propose(&w.ctx(&fake), &scope(), "yes, that one").await;
    assert_eq!(r.outcome, "ask");
    assert_eq!(r.targets.len(), 2);
    assert_eq!(r.run_id, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn unsure_or_a_weak_answer_asks_and_control_shows_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says(UNSURE, 0.8),
        says(&w.s(), 0.3),
        says(CONTROL, 0.9),
    ]);
    let ctx = w.ctx(&fake);
    assert_eq!(propose(&ctx, &scope(), MESSAGE).await.outcome, "ask");
    assert_eq!(propose(&ctx, &scope(), MESSAGE).await.outcome, "ask");
    assert_eq!(propose(&ctx, &scope(), MESSAGE).await.outcome, "none");
    assert_eq!(fake.calls(), 3);
}

#[tokio::test]
async fn shadow_records_and_shows_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&w.m(), 0.9)]);
    let r = propose(&w.ctx(&fake), &scope(), MESSAGE).await;
    assert_eq!(r.outcome, "none");
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, "shadow");
    // Nobody saw it: a follow-up never marks it.
    let s = w.store.lock().unwrap();
    assert!(!follow(&s, &scope(), runs[0].id, &w.m(), NOON).unwrap());
}

#[tokio::test]
async fn slash_commands_are_never_routed() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("m1", 0.9)]);
    let r = propose(
        &w.ctx(&fake),
        &scope(),
        "/compact keep the federation notes",
    )
    .await;
    assert_eq!(r.outcome, "none");
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn change_corrects_and_keeping_confirms_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.m(), 0.9), says(&w.m(), 0.9)]);
    let ctx = w.ctx(&fake);
    let kept = propose(&ctx, &scope(), MESSAGE).await.run_id.unwrap();
    let changed = propose(&ctx, &scope(), MESSAGE).await.run_id.unwrap();
    let s = w.store.lock().unwrap();
    assert!(follow(&s, &scope(), kept, &w.m(), NOON).unwrap());
    assert!(follow(&s, &scope(), changed, &w.s(), NOON).unwrap());
    // Decided once: a second follow-up changes nothing.
    assert!(!follow(&s, &scope(), changed, CONTROL, NOON).unwrap());
    let kept = s.get_decision_run(kept).unwrap().unwrap();
    let changed = s.get_decision_run(changed).unwrap().unwrap();
    assert_eq!(kept.followup.as_deref(), Some("confirmed"));
    assert_eq!(changed.followup.as_deref(), Some("corrected"));
    assert_eq!(changed.corrected_to, Some(w.s()));
}

#[tokio::test]
async fn a_follow_up_names_one_of_the_messages_targets() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.m(), 0.9)]);
    let run = propose(&w.ctx(&fake), &scope(), MESSAGE)
        .await
        .run_id
        .unwrap();
    let s = w.store.lock().unwrap();
    let e = follow(&s, &scope(), run, "s999", NOON).unwrap_err();
    assert_eq!(e.code, "E_INVALID");
}

#[test]
fn the_rule_reads_words_not_characters() {
    assert!(unclear("yes"));
    assert!(unclear("  do   it  now "));
    assert!(!unclear("rebase the federation branch"));
    assert!(is_command(" /clear"));
    assert!(!is_command("clear the queue"));
}

/// Review r15: a mission of an org that did not consent (D31) is offered to
/// the person but never named to the model; its goal stays home.
#[tokio::test]
async fn an_unconsenting_orgs_mission_is_never_sent() {
    let w = world();
    w.on("assist");
    let secret = {
        let s = w.store.lock().unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        let m = s
            .create_mission(
                &NewMission {
                    org_id: Some(org),
                    owner_person_id: None,
                    root_item_id: None,
                    name: "Acme payroll export",
                    goal: "Ship the payroll CSV for the Q3 audit.",
                    non_goals: None,
                    done_when: &[],
                    mode: None,
                    level: None,
                },
                "test",
            )
            .unwrap();
        s.set_mission_state(m.id, None, "active", "test").unwrap();
        format!("m{}", m.id)
    };
    let fake = Fake::answering(vec![says(&w.s(), 0.9)]);
    let r = propose(&w.ctx(&fake), &scope(), MESSAGE).await;
    // The person still sees all three targets.
    assert!(r.targets.iter().any(|t| t.option() == secret));
    let seen = fake.seen.lock().unwrap();
    let sent = serde_json::to_string(&seen[0]).unwrap();
    assert!(!sent.contains("payroll"), "{sent}");
    assert!(!sent.contains(&format!("\"{secret}\"")), "{sent}");
    assert!(sent.contains(&w.m()));
}

/// Review r15: only targets whose org consented are asked about; a target
/// with no org rides the message's own `unassigned` gate.
#[test]
fn only_consenting_orgs_targets_are_asked_about() {
    let w = world();
    w.on("assist");
    let s = w.store.lock().unwrap();
    let yes = s.add_org("Yes", None, false).unwrap().id;
    let no = s.add_org("No", None, false).unwrap().id;
    s.set_org_jev_allowed(yes, true).unwrap();
    let t = |id, org_id| Target {
        kind: TargetKind::Mission,
        id,
        name: "n".into(),
        detail: "d".into(),
        org_id,
    };
    let asked = consenting(&s, &[t(1, Some(yes)), t(2, Some(no)), t(3, None)]);
    let ids: Vec<i64> = asked.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![1, 3]);
}

/// Review r01: eight or more active missions do not hide every session.
#[test]
fn many_missions_leave_room_for_the_sessions() {
    let w = world();
    let s = w.store.lock().unwrap();
    for n in 0..10 {
        let m = s
            .create_mission(
                &NewMission {
                    name: Box::leak(format!("m{n}").into_boxed_str()),
                    goal: "g",
                    ..Default::default()
                },
                "test",
            )
            .unwrap();
        s.set_mission_state(m.id, None, "active", "test").unwrap();
    }
    let t = targets(&s, &scope()).unwrap();
    assert_eq!(t.len(), MAX_TARGETS);
    assert!(
        t.iter().any(|t| t.option() == w.s()),
        "the running session is still offered: {:?}",
        t.iter().map(Target::option).collect::<Vec<_>>()
    );
    assert_eq!(
        t.iter().filter(|t| t.kind == TargetKind::Mission).count(),
        MAX_TARGETS - 1
    );
}

/// Review r04: a follow-up on a run the caller cannot see marks nothing.
#[tokio::test]
async fn a_run_the_caller_cannot_see_is_not_followed() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.m(), 0.9)]);
    let run = propose(&w.ctx(&fake), &scope(), MESSAGE)
        .await
        .run_id
        .unwrap();
    let s = w.store.lock().unwrap();
    let stranger = s.create_person("eve", None).unwrap().id;
    // A paired device of a person who is not on the run, built the one way
    // a caller's scope is built (view_scope_tests pins that).
    let eve = crate::mcp::auth::Caller {
        api: None,
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id: 77,
            name: "eve's phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(stranger),
        }),
        mode: crate::mcp::auth::TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&s)
    .unwrap();
    assert!(!follow(&s, &eve, run, &w.m(), NOON).unwrap());
    assert!(s.get_decision_run(run).unwrap().unwrap().followup.is_none());
    assert!(
        follow(&s, &scope(), run, &w.m(), NOON).unwrap(),
        "its sender's reader still can"
    );
}
