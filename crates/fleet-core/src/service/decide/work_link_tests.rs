//! J1 `work_link` live (redesign 6.8) over a scripted backend: the gate,
//! shadow and assist, the R12 suggestion it makes, reuse of a decided run,
//! and the follow-ups a person's decision records. No test reaches
//! TypeSafe.

use super::work_link::*;
use super::*;
use crate::store::{Decider, DecisionRunFilter, DecisionRunRow, StartSource};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
const CONV: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";

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
    let other = if choice == NONE_OPTION {
        "i1"
    } else {
        NONE_OPTION
    };
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
    session: i64,
    /// `PAY-7` (refunds) and `PAY-9` (ledger), keyed local items.
    refunds: i64,
    ledger: i64,
}

/// A session three turns into [`CONV`], whose first prompt names no key,
/// and two fresh keyed local items. The feature as the defaults leave it.
fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let session = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.rebind_conversation(session, CONV, StartSource::Fleet, None, None)
        .unwrap();
    for _ in 0..crate::service::work::nudge::NUDGE_AFTER_TURNS {
        s.conversation_bump_turns(session, CONV).unwrap();
    }
    s.conversation_set_first_prompt(
        session,
        CONV,
        "the refund retries fail on the second attempt, see https://example.com/log and PAY-1",
    )
    .unwrap();
    let refunds = s
        .create_local_work_item(Some("PAY-7"), "Refund retries")
        .unwrap()
        .id;
    let ledger = s
        .create_local_work_item(Some("PAY-9"), "Ledger export")
        .unwrap()
        .id;
    World {
        store: Arc::new(Mutex::new(s)),
        session,
        refunds,
        ledger,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        // The items were written by the store's clock: ask on that clock.
        DecideCtx::new(Arc::clone(&self.store), fake.clone())
            .with_clock(Arc::new(crate::service::catalog::now_secs))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_WORK_LINK, mode).unwrap();
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
    fn links(&self) -> Vec<crate::store::WorkLinkRow> {
        self.store
            .lock()
            .unwrap()
            .session_work_links(self.session)
            .unwrap()
    }
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("i1", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Nothing);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
    assert!(w.links().is_empty());
}

#[tokio::test]
async fn shadow_records_the_stripped_question_and_suggests_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Recorded);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].mode.as_str(),
            runs[0].subject_kind.as_str(),
            runs[0].subject_id.clone(),
            runs[0].baseline_answer.as_deref(),
            runs[0].question_version.as_str(),
        ),
        (
            "shadow",
            SUBJECT_KIND,
            w.session.to_string(),
            Some(crate::store::DECISION_NO_BASELINE),
            QUESTION_VERSION,
        )
    );
    assert!(w.links().is_empty(), "shadow never suggests");
    // What was sent: the first prompt without its key and URL, and one
    // option per candidate plus none — the benchmark's question.
    let sent = fake.seen.lock().unwrap()[0].clone();
    let state = sent.state["first_prompt"].as_str().unwrap().to_string();
    assert!(state.contains("refund retries fail"), "{state}");
    assert!(
        !state.contains("PAY-1") && !state.contains("example.com"),
        "{state}"
    );
    let Question::Choice {
        criteria,
        instructions,
    } = sent.question
    else {
        panic!("a choice");
    };
    assert_eq!(instructions, serde_json::Value::String(INSTRUCTIONS.into()));
    let mut want = vec![
        option_id(w.refunds),
        option_id(w.ledger),
        NONE_OPTION.into(),
    ];
    want.sort();
    assert_eq!(criteria.keys().cloned().collect::<Vec<_>>(), want);
    // Every later prompt of the conversation reuses the decided run.
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Nothing);
    assert_eq!(fake.calls(), 1);
    // A person's decision never marks an answer nobody saw.
    let s = w.store.lock().unwrap();
    assert!(!record_decision(&s, w.session, Some(RULE), Some(w.refunds), true, 1).unwrap());
}

#[tokio::test]
async fn assist_makes_a_preselected_r12_suggestion_a_person_confirms() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.82)]);
    assert_eq!(
        ask(&w.ctx(&fake), w.session).await,
        Asked::Proposed {
            item_id: w.refunds,
            key: "PAY-7".into()
        }
    );
    let links = w.links();
    assert_eq!(links.len(), 1);
    let l = &links[0];
    assert_eq!(
        (
            l.state.as_str(),
            l.source.as_str(),
            l.rule.as_deref(),
            l.preselected
        ),
        ("suggested", "jev", Some(RULE), true)
    );
    // A live suggestion: nothing more to ask.
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Nothing);
    assert_eq!(fake.calls(), 1);

    let s = w.store.lock().unwrap();
    crate::service::work::detect::decide(&s, w.session, l.id, true, Decider::Person).unwrap();
    let run = s.list_decision_runs(&DecisionRunFilter::default()).unwrap()[0].clone();
    assert_eq!(run.followup.as_deref(), Some("confirmed"));
    let linked = s.session_work_links(w.session).unwrap();
    assert_eq!(linked[0].state, "confirmed");
    assert_eq!(linked[0].rule.as_deref(), Some(RULE), "why still reads");
}

#[tokio::test]
async fn a_rejected_proposal_is_labelled_and_never_proposed_again() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says(&option_id(w.refunds), 0.9),
        says(&option_id(w.refunds), 0.9),
    ]);
    ask(&w.ctx(&fake), w.session).await;
    let link = w.links()[0].id;
    {
        let s = w.store.lock().unwrap();
        crate::service::work::detect::decide(&s, w.session, link, false, Decider::Person).unwrap();
        assert_eq!(
            s.list_decision_runs(&DecisionRunFilter::default()).unwrap()[0]
                .followup
                .as_deref(),
            Some("rejected")
        );
    }
    // Asked again (the candidates changed: PAY-7 is out), the same answer
    // is not an option any more, so nothing can bring it back.
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Recorded);
    let sent = fake.seen.lock().unwrap()[1].clone();
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert!(!criteria.contains_key(&option_id(w.refunds)));
    assert!(w.links().iter().all(|l| l.state == "rejected"));
}

#[tokio::test]
async fn none_or_a_weak_answer_suggests_nothing() {
    for answer in [says(NONE_OPTION, 0.9), says("i1", 0.3)] {
        let w = world();
        w.on("assist");
        let fake = Fake::answering(vec![answer]);
        assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Recorded);
        assert!(w.links().is_empty());
    }
}

#[tokio::test]
async fn too_early_or_already_linked_is_not_asked() {
    // Two turns: the rules may still link it.
    let w = world();
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        s.conn_ref()
            .execute("UPDATE conversations SET turns = 2", [])
            .unwrap();
    }
    let fake = Fake::answering(vec![says("i1", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Nothing);
    // A person's link: nothing to ask.
    let w = world();
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        s.link_session_work(w.session, crate::store::WorkTarget::Key("PAY-9"), "manual")
            .unwrap();
    }
    assert_eq!(ask(&w.ctx(&fake), w.session).await, Asked::Nothing);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn confirming_another_link_corrects_the_proposal() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    ask(&w.ctx(&fake), w.session).await;
    let s = w.store.lock().unwrap();
    assert!(record_decision(&s, w.session, Some("R6"), Some(w.ledger), true, 5).unwrap());
    let run = s.list_decision_runs(&DecisionRunFilter::default()).unwrap()[0].clone();
    assert_eq!(run.followup.as_deref(), Some("corrected"));
    assert_eq!(run.corrected_to, Some(option_id(w.ledger)));
}

#[test]
fn options_name_items_and_nothing_else() {
    assert_eq!(option_id(12), "i12");
    assert_eq!(item_of("i12"), Some(12));
    assert_eq!(item_of(NONE_OPTION), None);
    assert_eq!(item_of("p12"), None);
}

#[tokio::test]
async fn review_names_jev_with_the_confidence_the_evidence_keeps() {
    use crate::service::work::view::proposer_of;
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.82)]);
    ask(&w.ctx(&fake), w.session).await;
    let l = w.links()[0].clone();
    let p = proposer_of(l.rule.as_deref(), &l.evidence).expect("Jev's");
    assert_eq!((p.source.as_str(), p.confidence_pct), ("jev", Some(82)));
    // A rule's own reading has no proposer.
    assert_eq!(proposer_of(Some("R6"), &l.evidence), None);
}
