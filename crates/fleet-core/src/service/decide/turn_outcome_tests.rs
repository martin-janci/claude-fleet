//! J2 `turn_outcome` over a scripted backend: the two consents, assist and
//! shadow, one decision per turn, hooks always winning (before and after
//! the answer), the follow-ups, J8's warning and what is sent. No test
//! reaches TypeSafe: the Stop is the store's own write (the Stop HOOK would
//! spawn the real backend), every other hook goes through `apply_hook`.

use super::turn_outcome::*;
use super::*;
use crate::mcp::hooks::HookPayload;
use crate::mcp::Caller;
use crate::service::attention::{self, Reason};
use crate::service::hooks::{apply_hook, HookContext};
use crate::ssh::SshClient;
use crate::store::{DecisionRunFilter, DecisionRunRow, SessionRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

/// An idle REPL after a reply that asks the person something.
const ASKS: &str = "⏺ I updated the parser and the tests pass.\n\n  Should I also update the \
                    migration guide?\n\n────────────────\n❯ \n────────────────\n  ⏵⏵ accept \
                    edits on (shift+tab to cycle) · ? for shortcuts";
/// A screen the pane rules cannot read (no footer they know).
const UNREADABLE: &str = "⏺ Done: the release notes are in CHANGES.md.\n\n» type here\n";

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
    let other = if choice == UNSURE { "finished" } else { UNSURE };
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

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
    session: i64,
}

/// A session on a host of org Acme whose turn just ended (the Stop's
/// write: idle, turn 1).
fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.upsert_host("hosta").unwrap();
    s.set_host_org("hosta", Some(org)).unwrap();
    let session = s
        .upsert_session("w", "hosta", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_claude_session_id(session, "uuid-1").unwrap();
    s.record_stop_hook_for_row(session).unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        org,
        session,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone())
    }
    /// The kill switch, the mode, a key and D31's consent — not D48's.
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_TURN_OUTCOME, mode).unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn replies(&self, on: bool) {
        let s = self.store.lock().unwrap();
        s.set_org_jev_reply_allowed(self.org, on).unwrap();
    }
    fn input(&self, tail: &str) -> TurnInput {
        let s = self.store.lock().unwrap();
        let row = s.get_session_by_id(self.session).unwrap().unwrap();
        let mut i = input_for(&s, self.session)
            .unwrap()
            .unwrap_or_else(|| panic!("a turn J2 reads: {row:?}"));
        i.tail = tail.into();
        i
    }
    fn row(&self) -> SessionRow {
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
    fn hook(&self, event: &str, notification: Option<&str>, prompt: Option<&str>) {
        let payload = HookPayload {
            session_id: Some("uuid-1".into()),
            hook_event_name: Some(event.into()),
            notification_type: notification.map(String::from),
            prompt: prompt.map(String::from),
            ..Default::default()
        };
        let caller = Caller::master();
        let ctx = HookContext {
            caller: &caller,
            pane_id: None,
            sync_start: false,
        };
        apply_hook(&self.store, &Arc::new(SshClient::new()), &payload, &ctx).unwrap();
    }
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    assert!(plan_for(&w.store, w.session).is_none());
    assert_eq!(ask(&w.ctx(&fake), &w.input(ASKS)).await, Asked::Skipped);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

/// D48: reply text needs its OWN consent, on top of D31's — per org, and
/// for a session with no org.
#[tokio::test]
async fn reply_text_needs_its_own_consent_on_top_of_d31() {
    let w = world();
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        // D31 alone lets the other features through, never this one.
        settings::set(&s, settings::DECIDE_JEV_START_PROJECT, "assist").unwrap();
        assert_eq!(
            gate(&s, Feature::StartProject, Some(w.org)),
            Ok(Mode::Assist)
        );
        assert_eq!(
            gate(&s, Feature::TurnOutcome, Some(w.org)),
            Err(Fallback::OrgOff)
        );
        // The reply consent alone is not enough either.
        s.set_org_jev_allowed(w.org, false).unwrap();
        s.set_org_jev_reply_allowed(w.org, true).unwrap();
        assert_eq!(
            gate(&s, Feature::TurnOutcome, Some(w.org)),
            Err(Fallback::OrgOff)
        );
        s.set_org_jev_allowed(w.org, true).unwrap();
        assert_eq!(
            gate(&s, Feature::TurnOutcome, Some(w.org)),
            Ok(Mode::Assist)
        );
        // No org: `unassigned` and `unassigned_reply`, both.
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        assert_eq!(gate(&s, Feature::TurnOutcome, None), Err(Fallback::OrgOff));
        assert_eq!(gate(&s, Feature::StartProject, None), Ok(Mode::Assist));
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED_REPLY, "true").unwrap();
        assert_eq!(gate(&s, Feature::TurnOutcome, None), Ok(Mode::Assist));
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "false").unwrap();
        assert_eq!(gate(&s, Feature::TurnOutcome, None), Err(Fallback::OrgOff));
    }
    // Without it nothing is sent.
    w.replies(false);
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input(ASKS)).await, Asked::Skipped);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn assist_sets_the_inbox_state_once_per_turn() {
    let w = world();
    w.on("assist");
    w.replies(true);
    assert_eq!(
        plan_for(&w.store, w.session).map(|p| p.0),
        Some(Mode::Assist)
    );
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    let ctx = w.ctx(&fake);
    let input = w.input(ASKS);
    let got = ask(&ctx, &input).await;
    let runs = w.runs();
    assert_eq!(
        got,
        Asked::Applied {
            outcome: "asked".into(),
            run_id: Some(runs[0].id)
        }
    );
    assert_eq!(runs[0].subject_kind, SUBJECT_KIND);
    assert_eq!(runs[0].subject_id, format!("{}:1", w.session));
    assert_eq!(
        runs[0].baseline_answer.as_deref(),
        Some("finished"),
        "the pane rules read an idle REPL"
    );
    let row = w.row();
    assert_eq!(row.turn_outcome.as_deref(), Some("asked"));
    let a = attention::needs_attention(&row).expect("Jev proposes it waits");
    // A proposal, kept apart from Needs you (gap plan G1.6).
    assert_eq!(a.reason, Reason::ProbablyWaiting);
    assert!(!a.state.counts_toward_badge());
    // The same turn is never asked again.
    assert_eq!(ask(&ctx, &input).await, Asked::Skipped);
    assert_eq!(fake.calls(), 1);
    // What was sent: the screen, the chrome left out.
    let sent = fake.seen.lock().unwrap()[0].clone();
    let screen = sent.state["screen"].as_str().unwrap();
    assert!(
        screen.contains("Should I also update the migration guide?"),
        "{screen}"
    );
    assert!(!screen.contains("shift+tab"), "{screen}");
    assert!(!screen.contains('❯'), "{screen}");
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec!["asked", "finished", "stuck", "unsure", "working"],
        "the five outcomes (sorted on the wire)"
    );
}

#[tokio::test]
async fn shadow_unsure_or_a_weak_answer_applies_nothing() {
    for (mode, answer) in [
        ("shadow", says("asked", 0.9)),
        ("assist", says(UNSURE, 0.9)),
        ("assist", says("asked", 0.3)),
    ] {
        let w = world();
        w.on(mode);
        w.replies(true);
        let fake = Fake::answering(vec![answer]);
        assert_eq!(ask(&w.ctx(&fake), &w.input(ASKS)).await, Asked::Recorded);
        assert_eq!(w.row().turn_outcome, None, "{mode}");
        assert_eq!(w.runs().len(), 1);
    }
}

/// The acceptance line: a hook event overrides a Jev answer — after it
/// (the hook clears it) and before it (the answer never lands).
#[tokio::test]
async fn a_hook_event_overrides_a_jev_answer() {
    // After: Jev said finished, then a permission dialog's Notification.
    let w = world();
    w.on("assist");
    w.replies(true);
    let fake = Fake::answering(vec![says("finished", 0.9)]);
    assert!(matches!(
        ask(&w.ctx(&fake), &w.input(ASKS)).await,
        Asked::Applied { .. }
    ));
    assert_eq!(w.row().turn_outcome.as_deref(), Some("finished"));
    w.hook("Notification", Some("permission_prompt"), None);
    let row = w.row();
    assert_eq!(row.turn_outcome, None, "the hook cleared Jev's answer");
    assert_eq!(row.claude_status.as_deref(), Some("blocked"));
    assert_eq!(
        attention::needs_attention(&row).map(|a| a.reason),
        Some(Reason::Waiting)
    );
    let r = &w.runs()[0];
    assert_eq!(
        (r.followup.as_deref(), r.corrected_to.as_deref()),
        (Some("corrected"), Some("asked")),
        "the hook said otherwise"
    );

    // Before: the Notification lands while Jev is still thinking.
    let w = world();
    w.on("assist");
    w.replies(true);
    let input = w.input(ASKS);
    w.hook("Notification", Some("permission_prompt"), None);
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &input).await, Asked::Recorded);
    assert_eq!(w.row().turn_outcome, None, "the answer never landed");
    assert_eq!(
        w.runs()[0].followup.as_deref(),
        Some("confirmed"),
        "the hook said the same"
    );

    // Any later hook clears it too: the next prompt, the next Stop.
    let w = world();
    w.on("assist");
    w.replies(true);
    let fake = Fake::answering(vec![says("stuck", 0.9), says("asked", 0.9)]);
    ask(&w.ctx(&fake), &w.input(ASKS)).await;
    assert_eq!(w.row().turn_outcome.as_deref(), Some("stuck"));
    assert_eq!(
        attention::needs_attention(&w.row()).map(|a| a.reason),
        Some(Reason::Stuck)
    );
    w.hook("UserPromptSubmit", None, Some("go on"));
    assert_eq!(w.row().turn_outcome, None);
    w.store
        .lock()
        .unwrap()
        .record_stop_hook_for_row(w.session)
        .unwrap();
    ask(&w.ctx(&fake), &w.input(ASKS)).await;
    assert_eq!(w.row().turn_outcome.as_deref(), Some("asked"), "turn 2");
    w.store
        .lock()
        .unwrap()
        .record_stop_hook_for_row(w.session)
        .unwrap();
    assert_eq!(w.row().turn_outcome, None, "a new Stop starts over");
}

#[tokio::test]
async fn a_persons_answer_confirms_asked_and_shadow_is_never_marked() {
    let w = world();
    w.on("assist");
    w.replies(true);
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    ask(&w.ctx(&fake), &w.input(ASKS)).await;
    w.hook("UserPromptSubmit", None, Some("yes, update it too"));
    assert_eq!(w.runs()[0].followup.as_deref(), Some("confirmed"));
    assert_eq!(w.row().turn_outcome, None);

    let w = world();
    w.on("shadow");
    w.replies(true);
    let fake = Fake::answering(vec![says("asked", 0.9)]);
    ask(&w.ctx(&fake), &w.input(ASKS)).await;
    w.hook("Notification", Some("permission_prompt"), None);
    w.hook("UserPromptSubmit", None, Some("yes"));
    assert_eq!(w.runs()[0].followup, None);
}

/// J8: when the pane rules read nothing on the screen, the run's baseline
/// is `none` and the timeline says so, once per turn.
#[tokio::test]
async fn j8_warns_when_the_rules_cannot_read_the_screen() {
    assert_eq!(rule_outcome(UNREADABLE), None);
    assert_eq!(rule_outcome(ASKS), Some("finished"));
    let w = world();
    w.on("shadow");
    w.replies(true);
    let fake = Fake::answering(vec![says("finished", 0.9)]);
    ask(&w.ctx(&fake), &w.input(UNREADABLE)).await;
    assert_eq!(w.runs()[0].baseline_answer.as_deref(), Some("none"));
    let unreadable = |w: &World| {
        w.store
            .lock()
            .unwrap()
            .list_session_events(w.session, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == PANE_UNREADABLE)
            .count()
    };
    assert_eq!(unreadable(&w), 1);
    ask(&w.ctx(&fake), &w.input(UNREADABLE)).await;
    assert_eq!(unreadable(&w), 1, "a decided turn is not asked again");
    // A screen the rules read raises nothing.
    let w = world();
    w.on("shadow");
    w.replies(true);
    let fake = Fake::answering(vec![says("finished", 0.9)]);
    ask(&w.ctx(&fake), &w.input(ASKS)).await;
    assert_eq!(unreadable(&w), 0);
}

#[test]
fn the_tail_is_stripped_bounded_and_its_code_replaced() {
    let mut raw = String::from("\u{1b}[1mold line\u{1b}[0m\n");
    for i in 0..100 {
        raw.push_str(&format!("line {i}\n"));
    }
    raw.push_str("```rust\nfn secret() {}\nlet x = 1;\n```\nDoes this look right?\n");
    raw.push_str("────\n❯ \n────\n  ? for shortcuts\n");
    let t = prepare_tail(&raw);
    assert!(!t.contains('\u{1b}'));
    assert!(!t.contains("old line"), "only the newest lines");
    assert_eq!(t.lines().count(), TAIL_LINES);
    assert!(t.contains("[code: rust, 2 lines]"), "{t}");
    assert!(!t.contains("secret"));
    assert!(t.ends_with("Does this look right?"), "{t}");
    let long = "x".repeat(TAIL_CHARS * 2);
    assert_eq!(prepare_tail(&long).chars().count(), TAIL_CHARS);
    assert_eq!(ends_with_question(&raw), Some("asked"));
    assert_eq!(ends_with_question(UNREADABLE), Some("finished"));
}

#[test]
fn a_hooks_effect_is_a_word() {
    use crate::service::pane_intel::{ClaudeStatus, StuckKind};
    assert_eq!(hook_word(ClaudeStatus::Blocked, None), Some("asked"));
    assert_eq!(
        hook_word(ClaudeStatus::Blocked, Some(Some(StuckKind::PressEnter))),
        Some("stuck")
    );
    assert_eq!(
        hook_word(ClaudeStatus::Working, Some(None)),
        Some("working")
    );
    assert_eq!(hook_word(ClaudeStatus::Idle, None), None);
}
