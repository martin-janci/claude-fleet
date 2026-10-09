//! K4 `duplicate` (redesign 6.9) over a scripted backend: the gate, shadow
//! and assist, the hint a proposal carries on the Work view, asking once,
//! and the follow-ups a person's Merge or Keep both records. No test
//! reaches TypeSafe.

use super::duplicate::*;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow, Proposal, WorkItemRow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

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
    /// `PAY-1` "Payments", the proposals' parent.
    parent: i64,
    /// `PAY-7` "Refund retries fail on second attempt", open.
    refunds: i64,
    /// `PAY-9` "Ledger export", open, nothing like the proposal.
    ledger: i64,
    /// "Fix refund retries", proposed under `parent`.
    proposal: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let parent = s
        .create_local_work_item(Some("PAY-1"), "Payments")
        .unwrap()
        .id;
    let refunds = s
        .create_local_work_item(Some("PAY-7"), "Refund retries fail on second attempt")
        .unwrap()
        .id;
    let ledger = s
        .create_local_work_item(Some("PAY-9"), "Ledger export")
        .unwrap()
        .id;
    let proposal = s
        .propose_subtask(&Proposal {
            parent_id: parent,
            title: "Fix refund retries",
            notes: None,
            why: Some("the second refund attempt errors out"),
            proposed_by: "dev @ h",
        })
        .unwrap()
        .id;
    World {
        store: Arc::new(Mutex::new(s)),
        parent,
        refunds,
        ledger,
        proposal,
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
        settings::set(&s, settings::DECIDE_JEV_DUPLICATE, mode).unwrap();
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
    fn detail(&self) -> crate::service::work::view::TaskDetail {
        crate::service::work::view::task(
            &self.store,
            &crate::service::view_scope::ViewScope::internal(),
            &format!("item:{}", self.parent),
        )
        .unwrap()
    }
}

fn row(id: i64, title: &str) -> WorkItemRow {
    WorkItemRow {
        id,
        source: "local".into(),
        key: Some(format!("K-{id}")),
        title: title.into(),
        url: None,
        status_category: "todo".into(),
        status_set_by: None,
        status_set_at: None,
        created_at: 0,
        updated_at: 0,
        tracker_id: None,
        external_id: None,
        aliases: vec![],
        kind: None,
        hierarchy_level: None,
        status_name: None,
        resolution: None,
        parent_id: None,
        assignees: vec![],
        iteration: None,
        updated_ext: None,
        status_changed_at: None,
        fetched_at: None,
        unavailable_at: None,
        unavailable_reason: None,
        origin: None,
        project_id: None,
        notes: None,
        task_id: None,
        proposal_state: None,
        proposed_by: None,
        proposal_why: None,
        held_at: None,
        done_when: vec![],
        due_at: None,
    }
}

#[test]
fn titles_compare_by_their_telling_words() {
    let w = title_words("Fix the Refund-retries, add tests");
    assert_eq!(
        w.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["refund", "retries", "tests"]
    );
    assert_eq!(overlap(&w, &title_words("refund retries")), 1.0);
    assert_eq!(overlap(&w, &title_words("ledger export")), 0.0);
    assert_eq!(overlap(&w, &title_words("fix it")), 0.0);
}

#[test]
fn rank_offers_alike_tasks_of_the_same_org_and_never_itself_or_its_parent() {
    let mut proposed = row(1, "Refund retries fail");
    proposed.parent_id = Some(2);
    let open = vec![
        row(1, "Refund retries fail"),
        row(2, "Refund work"),
        row(3, "Refund page"),
        row(4, "Ledger export"),
        row(5, "Refund retries fail twice"),
        row(6, "Refund retries elsewhere"),
    ];
    // 6 is another org's.
    let got: Vec<i64> = rank(&proposed, open, |i| i.id != 6)
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(got, vec![5, 3]);
    let many: Vec<WorkItemRow> = (10..40).map(|i| row(i, "refund")).collect();
    assert_eq!(rank(&proposed, many, |_| true).len(), MAX_CANDIDATES);
}

#[test]
fn the_question_names_each_task_and_none() {
    let q = question(&[row(7, "Refund retries")]);
    let Question::Choice { criteria, .. } = q else {
        panic!("a choice");
    };
    let keys: Vec<&String> = criteria.keys().collect();
    assert_eq!(keys, vec!["i7", "none"]);
    assert_eq!(item_of("i7"), Some(7));
    assert_eq!(item_of(NONE_OPTION), None);
}

#[tokio::test]
async fn off_by_default_nothing_is_asked() {
    let w = world();
    let fake = Fake::answering(vec![]);
    assert_eq!(ask(&w.ctx(&fake), w.proposal).await, Asked::Nothing);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.proposal).await, Asked::Recorded);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].feature, "duplicate");
    assert_eq!(runs[0].subject_kind, "work_item");
    assert_eq!(runs[0].subject_id, w.proposal.to_string());
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(NONE_OPTION));
    // Only the alike open task is offered: not the ledger, not the parent.
    let seen = fake.seen.lock().unwrap();
    let Question::Choice { criteria, .. } = &seen[0].question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec![option_id(w.refunds), NONE_OPTION.to_string()]
    );
    assert!(!criteria.contains_key(&option_id(w.ledger)));
    drop(seen);
    assert!(w.detail().proposals[0].duplicate.is_none());
}

#[tokio::test]
async fn assist_leaves_the_hint_on_the_proposal_and_asks_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.82)]);
    let ctx = w.ctx(&fake);
    assert_eq!(
        ask(&ctx, w.proposal).await,
        Asked::Proposed { item_id: w.refunds }
    );
    let d = w.detail();
    let hint = d.proposals[0].duplicate.clone().expect("a hint");
    assert_eq!(hint.item_id, w.refunds);
    assert_eq!(hint.key.as_deref(), Some("PAY-7"));
    assert_eq!(hint.task_id, format!("item:{}", w.refunds));
    assert_eq!(hint.source, "jev");
    assert_eq!(hint.confidence_pct, Some(82));
    // The same input is never asked again.
    assert_eq!(ask(&ctx, w.proposal).await, Asked::Nothing);
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn none_or_too_unsure_is_recorded_not_proposed() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(NONE_OPTION, 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.proposal).await, Asked::Recorded);
    assert!(w.detail().proposals[0].duplicate.is_none());

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.3)]);
    assert_eq!(ask(&w.ctx(&fake), w.proposal).await, Asked::Recorded);
    assert!(w.detail().proposals[0].duplicate.is_none());
}

#[tokio::test]
async fn nothing_alike_or_a_decided_proposal_is_not_asked() {
    let w = world();
    w.on("assist");
    let other = w
        .store
        .lock()
        .unwrap()
        .propose_subtask(&Proposal {
            parent_id: w.parent,
            title: "Onboarding emails",
            notes: None,
            why: None,
            proposed_by: "dev @ h",
        })
        .unwrap()
        .id;
    let fake = Fake::answering(vec![]);
    assert_eq!(ask(&w.ctx(&fake), other).await, Asked::Nothing);
    w.store
        .lock()
        .unwrap()
        .decide_proposal(w.proposal, true)
        .unwrap();
    assert_eq!(ask(&w.ctx(&fake), w.proposal).await, Asked::Nothing);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn merge_confirms_and_keep_both_rejects_through_a_persons_decision() {
    let args = |id| crate::service::work::WorkLinkArgs {
        action: "reject".into(),
        item_id: Some(id),
        ..Default::default()
    };
    // Merge: the person rejects the proposal.
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    ask(&w.ctx(&fake), w.proposal).await;
    crate::service::work::local::decide(
        &args(w.proposal),
        &w.store,
        &crate::service::orgs::OrgScope::All,
        false,
    )
    .unwrap();
    assert_eq!(w.runs()[0].followup.as_deref(), Some("confirmed"));

    // Keep both: the person accepts it.
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    ask(&w.ctx(&fake), w.proposal).await;
    crate::service::work::local::decide(
        &args(w.proposal),
        &w.store,
        &crate::service::orgs::OrgScope::All,
        true,
    )
    .unwrap();
    assert_eq!(w.runs()[0].followup.as_deref(), Some("rejected"));
}

#[tokio::test]
async fn a_shadow_answer_is_never_marked() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&option_id(w.refunds), 0.9)]);
    ask(&w.ctx(&fake), w.proposal).await;
    let s = w.store.lock().unwrap();
    assert!(!record_decision(&s, w.proposal, false, 1).unwrap());
    drop(s);
    assert_eq!(w.runs()[0].followup, None);
}
