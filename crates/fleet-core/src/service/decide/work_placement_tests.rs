//! K5 `work_placement` (redesign 6.9 part 2) over a scripted backend: the
//! gate, shadow and assist, the label the Work view puts back, asking only
//! where nothing placed the task, and the follow-up a person's placement
//! records. No test reaches TypeSafe.

use super::work_placement::*;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow, NativeItem, Placement, WorkRule};
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
    let other = if choice == UNSURE {
        NONE_OPTION
    } else {
        UNSURE
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
    /// "Refund retries fail", standalone, placed nowhere.
    task: i64,
}

/// Three placed tasks (two in "Payments", one in "Infra") and one new
/// standalone task. The feature as the defaults leave it.
fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let native = |title: &str| {
        s.create_native_item(&NativeItem {
            title,
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id
    };
    for (title, group) in [
        ("Ledger export", "Payments"),
        ("Card refunds", "Payments"),
        ("Disk alerts", "Infra"),
    ] {
        let id = native(title);
        s.set_work_placement(&format!("item:{id}"), Some(group), None, 0, "me")
            .unwrap();
    }
    let task = native("Refund retries fail");
    World {
        store: Arc::new(Mutex::new(s)),
        task,
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
        settings::set(&s, settings::DECIDE_JEV_WORK_PLACEMENT, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn opt(&self, label: &str) -> String {
        option_of(
            &self.store.lock().unwrap().decision_fp_key().unwrap(),
            label,
        )
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn proposed_group(&self) -> Option<String> {
        let d = crate::service::work::view::task(
            &self.store,
            &crate::service::view_scope::ViewScope::internal(),
            &format!("item:{}", self.task),
        )
        .unwrap();
        d.task
            .proposals
            .iter()
            .find(|p| p.feature == "work_placement")
            .map(|p| p.value.clone())
    }
    fn place(&self, group: &str) {
        crate::service::work::structure::place(
            &self.store,
            &crate::service::view_scope::ViewScope::internal(),
            &format!("item:{}", self.task),
            Some(group),
            None,
            Some(0),
            "me",
        )
        .unwrap();
    }
}

fn placement(task: &str, group: &str) -> Placement {
    Placement {
        task_id: task.into(),
        group: Some(group.into()),
        note: None,
        version: 1,
        updated_at: 0,
        updated_by: None,
    }
}

#[test]
fn groups_are_the_used_labels_most_used_first_then_rule_groups() {
    let ps = [
        placement("item:1", "Infra"),
        placement("item:2", "Payments"),
        placement("item:3", " Payments "),
        placement("item:4", "  "),
    ];
    let rule = |group: &str, enabled: bool| WorkRule {
        id: 1,
        name: group.into(),
        enabled,
        version: 1,
        conditions: Default::default(),
        group: group.into(),
        host_alias: None,
        profile: None,
        created_at: 0,
        updated_at: 0,
    };
    assert_eq!(
        groups(
            &ps,
            &[
                rule("Billing", true),
                rule("Infra", true),
                rule("Old", false)
            ]
        ),
        vec!["Payments", "Infra", "Billing"]
    );
    let many: Vec<Placement> = (0..40)
        .map(|i| placement(&format!("item:{i}"), &format!("G{i:02}")))
        .collect();
    assert_eq!(groups(&many, &[]).len(), MAX_GROUPS);
}

#[test]
fn an_option_never_carries_the_label() {
    let k = Secret::new("k".repeat(64));
    let o = option_of(&k, "Payments");
    assert!(o.starts_with('g') && o.len() == 13, "{o}");
    assert!(!o.contains("Payments"));
    assert_ne!(o, option_of(&k, "Infra"));
}

#[tokio::test]
async fn off_by_default_nothing_is_asked() {
    let w = world();
    let fake = Fake::answering(vec![]);
    assert_eq!(ask(&w.ctx(&fake), w.task).await, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_the_option_word_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&w.opt("Payments"), 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.task).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].feature, "work_placement");
    assert_eq!(runs[0].answer.as_deref(), Some(w.opt("Payments").as_str()));
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(NONE_OPTION));
    let seen = fake.seen.lock().unwrap();
    let Question::Choice { criteria, .. } = &seen[0].question else {
        panic!("a choice");
    };
    assert_eq!(criteria.len(), 4, "Payments, Infra, none, unsure");
    drop(seen);
    assert_eq!(w.proposed_group(), None);
}

#[tokio::test]
async fn assist_proposes_the_group_by_its_label_and_asks_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.opt("Payments"), 0.8)]);
    let ctx = w.ctx(&fake);
    assert_eq!(ask(&ctx, w.task).await.as_deref(), Some("Payments"));
    assert_eq!(w.proposed_group().as_deref(), Some("Payments"));
    assert_eq!(ask(&ctx, w.task).await, None);
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn unsure_none_or_a_group_gone_since_proposes_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(UNSURE, 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.task).await, None);
    assert_eq!(w.proposed_group(), None);

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.opt("Infra"), 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.task).await.as_deref(), Some("Infra"));
    // The one task in "Infra" moves: no group answers to the option now.
    let s = w.store.lock().unwrap();
    let infra = s
        .work_placements()
        .unwrap()
        .into_iter()
        .find(|p| p.group.as_deref() == Some("Infra"))
        .unwrap();
    s.set_work_placement(&infra.task_id, Some("Ops"), None, infra.version, "me")
        .unwrap();
    drop(s);
    assert_eq!(w.proposed_group(), None);
}

#[tokio::test]
async fn a_placed_task_or_a_subtask_is_not_asked() {
    let w = world();
    w.on("assist");
    w.place("Infra");
    let fake = Fake::answering(vec![]);
    assert_eq!(ask(&w.ctx(&fake), w.task).await, None);
    let sub = w
        .store
        .lock()
        .unwrap()
        .create_native_item(&NativeItem {
            title: "Retry backoff",
            parent_id: Some(w.task),
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id;
    assert_eq!(ask(&w.ctx(&fake), sub).await, None);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn a_persons_placement_confirms_or_corrects() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.opt("Payments"), 0.9)]);
    ask(&w.ctx(&fake), w.task).await;
    w.place("Payments");
    assert_eq!(w.runs()[0].followup.as_deref(), Some("confirmed"));
    assert_eq!(w.proposed_group(), None, "decided: no longer proposed");

    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&w.opt("Payments"), 0.9)]);
    ask(&w.ctx(&fake), w.task).await;
    w.place("Infra");
    let r = &w.runs()[0];
    assert_eq!(r.followup.as_deref(), Some("corrected"));
    assert_eq!(r.corrected_to.as_deref(), Some(w.opt("Infra").as_str()));
}

#[tokio::test]
async fn a_shadow_answer_is_never_marked() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&w.opt("Payments"), 0.9)]);
    ask(&w.ctx(&fake), w.task).await;
    w.place("Payments");
    assert_eq!(w.runs()[0].followup, None);
}
