//! J7 `tracker_duplicate` (redesign 6.8) over a scripted backend: the rule
//! first (a key in the title, nothing alike, never another local task),
//! shadow and assist, and the proposal on a Review suggestion. No test
//! reaches TypeSafe.

use super::tracker_duplicate::*;
use super::*;
use crate::service::work::view::review;
use crate::store::{
    DecisionRunFilter, DecisionRunRow, TrackerConfig, TrackerItemWrite, WorkItemRow,
};
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

fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == NONE_OPTION {
        UNSURE
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
        "usage": { "input_tokens": 70, "output_tokens": 2 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    retry_ticket: i64,
    local: i64,
    session: i64,
}

/// A tracker (no org) with PAY-31 and PAY-34; a local task LOC-7 a person
/// just made; a session whose prompt names LOC-7, so Review suggests it.
fn world(local_title: &str) -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h1").unwrap();
    let t = s
        .add_tracker("jira", "Jira", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["PAY".into(), "LOC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let mut ids = Vec::new();
    for (ext, key, title) in [
        ("31", "PAY-31", "Retry failed card payments with backoff"),
        ("34", "PAY-34", "Refund webhook times out"),
    ] {
        ids.push(
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
            .unwrap()
            .id,
        );
    }
    let local = s
        .create_local_work_item(Some("LOC-7"), local_title)
        .unwrap()
        .id;
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let session = s
        .upsert_session("one", "h1", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    crate::service::work::detect::on_prompt(&s, session, "work on LOC-7 and PAY-34 next", false)
        .unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        retry_ticket: ids[0],
        local,
        session,
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
        settings::set(&s, settings::DECIDE_JEV_TRACKER_DUPLICATE, mode).unwrap();
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
    fn local_suggestion(&self) -> crate::service::work::view::ReviewItem {
        review(
            &self.store,
            &crate::service::view_scope::ViewScope::internal(),
            None,
            None,
        )
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.session_id == self.session && i.task.key.as_deref() == Some("LOC-7"))
        .expect("Review suggests LOC-7")
    }
}

fn item(id: i64, title: &str, key: Option<&str>, tracker: Option<i64>) -> WorkItemRow {
    serde_json::from_value(serde_json::json!({
        "id": id, "source": if tracker.is_some() { "jira" } else { "local" },
        "key": key, "title": title, "tracker_id": tracker,
        "created_at": 0, "updated_at": 0,
    }))
    .unwrap()
}

#[test]
fn the_rule_reads_a_key_and_only_tracker_tickets_are_candidates() {
    let local = item(1, "PAY-32: CSV export tweaks", None, None);
    let open = vec![
        item(32, "CSV export for invoices", Some("PAY-32"), Some(1)),
        item(50, "CSV export tweaks", Some("LOC-50"), None),
    ];
    let found = candidates(&local, open, |_| true);
    assert_eq!(found.iter().map(|i| i.id).collect::<Vec<_>>(), vec![32]);
    assert_eq!(rule(&local, &found).as_deref(), Some("i32"));
    let other = item(2, "Write the onboarding guide", None, None);
    assert_eq!(rule(&other, &[]).as_deref(), Some(NONE_OPTION));
    let alike = item(3, "Export invoices to CSV", None, None);
    let found = candidates(
        &alike,
        vec![item(32, "CSV export for invoices", Some("PAY-32"), Some(1))],
        |_| true,
    );
    assert_eq!(rule(&alike, &found), None, "the model's");
}

#[tokio::test]
async fn off_by_default_nothing_is_asked() {
    let w = world("Retry declined payments with exponential backoff");
    let fake = Arc::new(Fake::default());
    assert_eq!(ask(&w.ctx(&fake), w.local).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_and_shows_nothing() {
    let w = world("Retry declined payments with exponential backoff");
    w.on("shadow");
    let fake = Arc::new(Fake::default());
    fake.script
        .lock()
        .unwrap()
        .push_back(says(&format!("i{}", w.retry_ticket), 0.9));
    assert_eq!(ask(&w.ctx(&fake), w.local).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(NONE_OPTION));
    assert!(w.local_suggestion().duplicate_of.is_none());
}

/// Assist: the local task's suggestion in Review carries "May duplicate
/// PAY-31 · Proposed by Jev"; nothing is linked or merged by itself.
#[tokio::test]
async fn assist_flags_the_local_task_in_review() {
    let w = world("Retry declined payments with exponential backoff");
    w.on("assist");
    let fake = Arc::new(Fake::default());
    fake.script
        .lock()
        .unwrap()
        .push_back(says(&format!("i{}", w.retry_ticket), 0.77));
    assert_eq!(ask(&w.ctx(&fake), w.local).await, Some(w.retry_ticket));
    let sent = fake.seen.lock().unwrap()[0].clone();
    let Question::Choice { criteria, .. } = &sent.question else {
        panic!("a choice");
    };
    assert!(criteria.contains_key(UNSURE) && criteria.contains_key(NONE_OPTION));
    let it = w.local_suggestion();
    let d = it.duplicate_of.expect("flagged");
    assert_eq!(
        (
            d.key.as_deref(),
            d.item_id,
            d.source.as_str(),
            d.confidence_pct
        ),
        (Some("PAY-31"), w.retry_ticket, "jev", Some(77))
    );
    assert_eq!(it.kind, "suggestion", "still a person's to decide");
    // Asked once per input.
    assert_eq!(ask(&w.ctx(&fake), w.local).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn none_unsure_or_weak_flags_nothing() {
    for (choice, conf) in [(NONE_OPTION, 0.9), (UNSURE, 0.9), ("i1", 0.3)] {
        let w = world("Retry declined payments with exponential backoff");
        w.on("assist");
        let fake = Arc::new(Fake::default());
        let choice = if choice == "i1" {
            format!("i{}", w.retry_ticket)
        } else {
            choice.to_string()
        };
        fake.script.lock().unwrap().push_back(says(&choice, conf));
        assert_eq!(ask(&w.ctx(&fake), w.local).await, None, "{choice}");
        assert!(w.local_suggestion().duplicate_of.is_none(), "{choice}");
    }
}

#[tokio::test]
async fn nothing_alike_is_never_asked() {
    let w = world("Write the onboarding guide");
    w.on("assist");
    let fake = Arc::new(Fake::default());
    assert_eq!(ask(&w.ctx(&fake), w.local).await, None);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(w.runs().is_empty());
}
