//! J6 `main_ticket` (redesign 6.8) over a scripted backend: the rule first
//! (one key, a branch naming one, a dump of keys), shadow and assist, the
//! proposal in Review, and a person's Confirm as its follow-up. No test
//! reaches TypeSafe.

use super::main_ticket::*;
use super::*;
use crate::service::work::view::{review, ReviewItem};
use crate::store::{
    DecisionRunFilter, DecisionRunRow, StartSource, TrackerConfig, TrackerItemWrite,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
const PROMPT: &str = "Fix TK-3: sessions time out after five minutes. TK-1 was the first try.";

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

fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == UNSURE { "k0" } else { UNSURE };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 80, "output_tokens": 2 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    session: i64,
}

/// A tracker with TK-1..3 (no org), one session whose first prompt names
/// TK-3 and TK-1: detection leaves two suggestions.
fn world(prompt: &str) -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h1").unwrap();
    let t = s
        .add_tracker("jira", "Jira", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["TK".into()],
            ..Default::default()
        },
    )
    .unwrap();
    for (ext, key, title) in [
        ("1", "TK-1", "Session timeout, first attempt"),
        ("2", "TK-2", "Audit log"),
        ("3", "TK-3", "Sessions time out after five minutes"),
    ] {
        s.upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: ext.into(),
                key: Some(key.into()),
                title: title.into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let id = s
        .upsert_session("one", "h1", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let conv = "00000001-cccc-cccc-cccc-cccccccccccc";
    s.rebind_conversation(id, conv, StartSource::Fleet, None, None)
        .unwrap();
    s.conversation_set_first_prompt(id, conv, prompt).unwrap();
    crate::service::work::detect::on_prompt(&s, id, prompt, true).unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        session: id,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone())
            .with_clock(Arc::new(crate::service::catalog::now_secs))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_MAIN_TICKET, mode).unwrap();
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
    fn keys(&self) -> Vec<KeyCandidate> {
        suggested_keys(&self.store.lock().unwrap(), self.session).unwrap()
    }
    fn option(&self, key: &str) -> String {
        self.keys()
            .into_iter()
            .find(|k| k.key == key)
            .unwrap_or_else(|| panic!("no suggestion for {key}"))
            .option
    }
    fn review(&self) -> Vec<ReviewItem> {
        review(
            &self.store,
            &crate::service::view_scope::ViewScope::internal(),
            None,
            None,
        )
        .unwrap()
        .items
    }
}

fn kc(i: i64, key: &str) -> KeyCandidate {
    KeyCandidate {
        option: option_of(i),
        key: key.into(),
        title: format!("title of {key}"),
    }
}

#[test]
fn the_rule_decides_one_key_a_branch_and_a_dump() {
    assert_eq!(rule(&[], None).as_deref(), Some(UNSURE));
    assert_eq!(rule(&[kc(1, "PAY-1")], None).as_deref(), Some("k1"));
    let two = [kc(1, "PAY-1"), kc(2, "PAY-12")];
    assert_eq!(rule(&two, None), None, "the model's");
    assert_eq!(
        rule(&two, Some("fix/pay-12-retry")).as_deref(),
        Some("k2"),
        "PAY-1 is not in PAY-12"
    );
    assert_eq!(rule(&two, Some("pay-1-and-pay-12")), None, "both named");
    let dump: Vec<KeyCandidate> = (1..=9).map(|i| kc(i, &format!("PAY-{i}"))).collect();
    assert_eq!(rule(&dump, None).as_deref(), Some(UNSURE));
}

#[test]
fn the_prompt_goes_out_with_placeholders_for_the_keys() {
    let keys = [kc(7, "PAY-1"), kc(8, "PAY-12")];
    let m = masked(
        "Fix pay-12 now; PAY-1 was first. PAY-123 is unrelated.",
        &keys,
    );
    assert_eq!(m, "Fix [K2] now; [K1] was first. PAY-123 is unrelated.");
    let r = request("Fix PAY-12", &keys);
    let Question::Choice { criteria, .. } = &r.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["k7", "k8", UNSURE]
    );
    assert!(!r.state.to_string().contains("PAY-12"), "{}", r.state);
}

#[tokio::test]
async fn off_by_default_nothing_is_asked() {
    let w = world(PROMPT);
    assert_eq!(w.keys().len(), 2, "two suggestions");
    let fake = Arc::new(Fake::default());
    assert_eq!(ask(&w.ctx(&fake), w.session).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world(PROMPT);
    w.on("shadow");
    let fake = Arc::new(Fake::default());
    fake.script
        .lock()
        .unwrap()
        .push_back(says(&w.option("TK-3"), 0.9));
    assert_eq!(ask(&w.ctx(&fake), w.session).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, "shadow");
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(UNSURE));
    assert!(w.review().iter().all(|i| i.proposed_by.is_none()));
}

/// Assist: Review marks the proposed suggestion "Proposed by Jev · main
/// ticket among 2 keys", and only that one; asked once per input; a
/// person's Confirm of it marks the run confirmed.
#[tokio::test]
async fn assist_marks_the_main_ticket_in_review() {
    let w = world(PROMPT);
    w.on("assist");
    let fake = Arc::new(Fake::default());
    let main = w.option("TK-3");
    fake.script.lock().unwrap().push_back(says(&main, 0.82));
    let link = ask(&w.ctx(&fake), w.session).await.expect("a proposal");
    assert_eq!(option_of(link), main);
    let sent = fake.seen.lock().unwrap()[0].state.to_string();
    assert!(!sent.contains("TK-3") && sent.contains("[K"), "{sent}");

    let items = w.review();
    let marked: Vec<&ReviewItem> = items.iter().filter(|i| i.proposed_by.is_some()).collect();
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].link_id, link);
    let p = marked[0].proposed_by.as_ref().unwrap();
    assert_eq!(
        (p.source.as_str(), p.reason.as_str(), p.confidence_pct),
        ("jev", "main ticket among 2 keys", Some(82))
    );

    // The same input is not asked again.
    assert_eq!(ask(&w.ctx(&fake), w.session).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

    {
        let s = w.store.lock().unwrap();
        crate::service::work::detect::decide(
            &s,
            w.session,
            link,
            true,
            crate::store::Decider::Person,
        )
        .unwrap();
    }
    let run = &w.runs()[0];
    assert_eq!(run.followup.as_deref(), Some("confirmed"));
}

/// Unsure, or under the floor, proposes nothing: nothing is pre-selected.
#[tokio::test]
async fn unsure_or_weak_proposes_nothing() {
    for weak in [false, true] {
        let w = world(PROMPT);
        w.on("assist");
        let fake = Arc::new(Fake::default());
        let answer = if weak {
            says(&w.option("TK-1"), 0.3)
        } else {
            says(UNSURE, 0.9)
        };
        fake.script.lock().unwrap().push_back(answer);
        assert_eq!(ask(&w.ctx(&fake), w.session).await, None, "weak {weak}");
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert!(w.review().iter().all(|i| i.proposed_by.is_none()));
    }
}

#[tokio::test]
async fn a_branch_naming_one_key_decides_without_a_call() {
    let w = world(PROMPT);
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        let row = s.get_session_by_id(w.session).unwrap().unwrap();
        let wt = s
            .upsert_worktree_on(
                "h1",
                row.project_id.unwrap(),
                "tk-3",
                "/src/api/.wt/tk-3",
                Some("fix/TK-3-timeout"),
            )
            .unwrap();
        s.link_session_worktree(w.session, wt).unwrap();
    }
    let fake = Arc::new(Fake::default());
    assert_eq!(ask(&w.ctx(&fake), w.session).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(w.runs().is_empty(), "the rule records nothing");
}
