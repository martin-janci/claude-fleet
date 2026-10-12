//! N3 `sibling_repos` over a scripted backend: the question and state, the
//! gate, assist and shadow, reuse of a decided run, follow-ups, and the
//! start preview's `suggested_sibling`. No test reaches TypeSafe.

use super::sibling_repos::*;
use super::start_project::Candidate;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow, WorkTarget};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";
/// Noon, UTC.
const NOON: i64 = 1_790_510_400;

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
    let other = if choice == NONE { UNSURE } else { NONE };
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

fn cand(project_id: i64, repo: &str) -> Candidate {
    Candidate {
        project_id,
        owner: "acme".into(),
        repo: repo.into(),
    }
}

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    World {
        store: Arc::new(Mutex::new(s)),
        org,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOON))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_SIBLING_REPOS, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn input(&self) -> SiblingInput {
        SiblingInput {
            key: "PAY-12".into(),
            title: "Refund button on the POS checkout screen".into(),
            item_id: Some(7),
            org_id: Some(self.org),
            description: Some("The cashier needs a refund action next to Pay.".into()),
            chosen: cand(1, "pos-frontend"),
            candidates: vec![cand(2, "payments-api"), cand(3, "docs")],
        }
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn record(&self, primary: i64, started: &[i64]) -> bool {
        let s = self.store.lock().unwrap();
        record_start(&s, Some(7), "PAY-12", primary, started, NOON).unwrap()
    }
}

#[test]
fn the_question_and_state_name_the_task_and_repositories_only() {
    let w = world();
    let mut input = w.input();
    input.description = Some("x".repeat(DESCRIPTION_CHARS + 50));
    let req = question_for(&input);
    assert_eq!(req, question_for(&input), "pure");
    let Question::Choice { criteria, .. } = &req.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec![NONE, "p2", "p3", UNSURE]
    );
    assert_eq!(req.state["task"]["key"], "PAY-12");
    assert_eq!(
        req.state["task"]["description"].as_str().unwrap().len(),
        DESCRIPTION_CHARS
    );
    assert_eq!(req.state["chosen"], "acme/pos-frontend");
    assert_eq!(
        req.state["candidates"],
        serde_json::json!(["acme/payments-api", "acme/docs"])
    );
    let keys: Vec<&String> = req.state.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["candidates", "chosen", "task"], "nothing else");
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    let ctx = w.ctx(&fake);
    assert_eq!(mode_for(&ctx, &w.input()), None);
    assert_eq!(ask(&ctx, &w.input()).await, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn assist_suggests_a_candidate_and_reuses_the_decided_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    let ctx = w.ctx(&fake);
    assert_eq!(mode_for(&ctx, &w.input()), Some(Mode::Assist));
    let got = ask(&ctx, &w.input()).await.expect("a suggestion");
    assert_eq!((got.project_id, got.confidence_pct), (2, Some(90)));
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(got.run_id, Some(runs[0].id));
    assert_eq!(
        (
            runs[0].feature.as_str(),
            runs[0].subject_kind.as_str(),
            runs[0].subject_id.as_str(),
            runs[0].question_version.as_str()
        ),
        ("sibling_repos", SUBJECT_KIND, "item:7", QUESTION_VERSION)
    );
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(NONE));
    // The dialog opened again on the same task: no second call.
    let again = ask(&ctx, &w.input()).await.expect("reused");
    assert_eq!(again.project_id, 2);
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn none_unsure_or_a_weak_answer_suggests_nothing() {
    for (answer, confidence) in [(NONE, 0.9), (UNSURE, 0.9), ("p2", 0.3)] {
        let w = world();
        w.on("assist");
        let fake = Fake::answering(vec![says(answer, confidence)]);
        assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None, "{answer}");
        assert_eq!(fake.calls(), 1);
        if confidence < MIN_CONFIDENCE {
            assert_eq!(w.runs()[0].fallback.as_deref(), Some("low_confidence"));
        }
        // Nothing proposed: a start marks nothing.
        assert!(!w.record(1, &[1]), "{answer}");
    }
}

#[tokio::test]
async fn shadow_records_and_suggests_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), &w.input()).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].mode.as_str(), runs[0].answer.as_deref()),
        ("shadow", Some("p2"))
    );
    // A person's start never marks an answer nobody saw.
    assert!(!w.record(1, &[1, 2]));
    assert_eq!(w.runs()[0].followup, None);
}

#[tokio::test]
async fn a_persons_start_with_the_proposed_sibling_confirms_it_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    ask(&w.ctx(&fake), &w.input()).await.unwrap();
    assert!(w.record(1, &[1, 2]));
    assert!(!w.record(1, &[1]), "a follow-up is never overwritten");
    let r = &w.runs()[0];
    assert_eq!(
        (r.followup.as_deref(), r.corrected_to.as_deref()),
        (Some("confirmed"), None)
    );
}

#[tokio::test]
async fn a_persons_start_without_it_corrects_to_theirs_or_none() {
    // Another sibling ticked: corrected to it.
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    ask(&w.ctx(&fake), &w.input()).await.unwrap();
    assert!(w.record(1, &[1, 3]));
    let r = &w.runs()[0];
    assert_eq!(
        (r.followup.as_deref(), r.corrected_to.as_deref()),
        (Some("corrected"), Some("p3"))
    );
    // Only the chosen project: corrected to none.
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("p2", 0.9)]);
    ask(&w.ctx(&fake), &w.input()).await.unwrap();
    assert!(w.record(1, &[1]));
    let r = &w.runs()[0];
    assert_eq!(
        (r.followup.as_deref(), r.corrected_to.as_deref()),
        (Some("corrected"), Some(NONE))
    );
}

#[tokio::test]
async fn no_candidates_asks_nothing() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![]);
    let input = SiblingInput {
        candidates: vec![],
        ..w.input()
    };
    assert_eq!(mode_for(&w.ctx(&fake), &input), None);
    assert_eq!(ask(&w.ctx(&fake), &input).await, None);
    assert_eq!(fake.calls(), 0);
}

// --- the start preview and the start's follow-up ----------------------------

use crate::service::trackers::tickets::{self, StartArgs};
use crate::service::view_scope::ViewScope;
use crate::store::Decider;

/// A fleet where `PAY-12` ran in `api` before (an ended link) and runs in
/// `docs` now, and is about to start in `app`.
struct Fleet {
    w: World,
    item: i64,
    app: i64,
    api: i64,
    docs: i64,
}

fn fleet() -> Fleet {
    let w = world();
    let (item, app, api, docs) = {
        let s = w.store.lock().unwrap();
        s.upsert_host("h").unwrap();
        let item = s
            .create_local_work_item(Some("PAY-12"), "Refund button")
            .unwrap()
            .id;
        let app = s.upsert_project("acme", "app", "/p/acme/app").unwrap();
        let api = s.upsert_project("acme", "api", "/p/acme/api").unwrap();
        let docs = s.upsert_project("acme", "docs", "/p/acme/docs").unwrap();
        let on = |name: &str, pid: i64| {
            let sid = s
                .upsert_session(name, "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                    [pid, sid],
                )
                .unwrap();
            s.link_session_work(sid, WorkTarget::Item(item), "manual")
                .unwrap();
            sid
        };
        let old = on("old", api);
        on("now", docs);
        s.delete_session(old).unwrap();
        (item, app, api, docs)
    };
    Fleet {
        w,
        item,
        app,
        api,
        docs,
    }
}

impl Fleet {
    fn args(&self, project: Option<i64>) -> StartArgs {
        StartArgs {
            item_id: Some(self.item),
            project_id: project,
            host_alias: Some("h".into()),
            ..Default::default()
        }
    }
    async fn preview(&self, fake: &Arc<Fake>, project: Option<i64>) -> tickets::StartPreview {
        self.preview_with(fake, self.args(project)).await
    }
    async fn preview_with(&self, fake: &Arc<Fake>, args: StartArgs) -> tickets::StartPreview {
        let net = crate::service::trackers::TrackerNet::fake(Arc::new(
            crate::net::https::FakeTransport::new(),
        ));
        tickets::preview_start_decided(
            &self.w.store,
            &args,
            &ViewScope::internal(),
            &net,
            Some(&self.w.ctx(fake)),
        )
        .await
        .unwrap()
    }
}

#[test]
fn the_candidates_are_the_keys_past_and_live_projects_but_the_chosen_one() {
    let f = fleet();
    let s = f.w.store.lock().unwrap();
    let ids = |chosen| -> Vec<i64> {
        tickets::sibling_candidates(&s, "PAY-12", None, chosen)
            .unwrap()
            .into_iter()
            .map(|c| c.project_id)
            .collect()
    };
    let got = ids(f.app);
    got.iter()
        .for_each(|p| assert!([f.api, f.docs].contains(p)));
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(ids(f.docs), vec![f.api], "never the chosen project");
    assert!(tickets::sibling_candidates(&s, "NEW-1", None, f.app)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_preview_carries_a_sibling_only_with_a_planned_project() {
    let f = fleet();
    f.w.on("assist");
    let fake = Fake::answering(vec![says(&format!("p{}", f.api), 0.9)]);
    // No host picked: nothing planned, nothing asked for siblings.
    let p = f
        .preview_with(
            &fake,
            StartArgs {
                host_alias: None,
                ..f.args(Some(f.app))
            },
        )
        .await;
    assert_eq!(p.missing.as_deref(), Some("host"));
    assert_eq!(p.plan, None);
    assert_eq!(p.suggested_sibling, None);
    assert_eq!(fake.calls(), 0);
    // Planned in app: asked over api and docs.
    let p = f.preview(&fake, Some(f.app)).await;
    assert!(p.plan.is_some(), "{:?}", p.missing);
    let got = p.suggested_sibling.clone().expect("a sibling");
    assert_eq!(got.project_id, f.api);
    assert_eq!(fake.calls(), 1);
    let sent = fake.seen.lock().unwrap()[0].clone();
    assert_eq!(sent.state["chosen"], "acme/app");
    let json = serde_json::to_value(&p.suggested_sibling).unwrap();
    assert_eq!(json["project_id"], f.api);
}

#[tokio::test]
async fn the_preview_asks_nothing_without_candidates_or_with_the_defaults() {
    let f = fleet();
    let fake = Fake::answering(vec![says("p1", 0.9)]);
    // The defaults: the gate refuses.
    let p = f.preview(&fake, Some(f.app)).await;
    assert_eq!(p.suggested_sibling, None);
    assert!(
        !serde_json::to_string(&p)
            .unwrap()
            .contains("suggested_sibling"),
        "absent on the wire"
    );
    // On, but the key never ran anywhere else.
    f.w.on("assist");
    {
        let s = f.w.store.lock().unwrap();
        s.create_local_work_item(Some("NEW-1"), "Fresh").unwrap();
    }
    let net = crate::service::trackers::TrackerNet::fake(Arc::new(
        crate::net::https::FakeTransport::new(),
    ));
    let p = tickets::preview_start_decided(
        &f.w.store,
        &StartArgs {
            reference: Some("NEW-1".into()),
            project_id: Some(f.app),
            host_alias: Some("h".into()),
            ..Default::default()
        },
        &ViewScope::internal(),
        &net,
        Some(&f.w.ctx(&fake)),
    )
    .await
    .unwrap();
    assert!(p.plan.is_some());
    assert_eq!(p.suggested_sibling, None);
    assert_eq!(fake.calls(), 0);
}

/// `start_many`'s follow-up: a person's multi-repo start that ticked the
/// proposed sibling confirms it; an agent's marks nothing.
#[tokio::test]
async fn a_multi_start_confirms_and_an_agents_start_marks_nothing() {
    for (decider, want) in [(Decider::Agent, None), (Decider::Person, Some("confirmed"))] {
        let f = fleet();
        f.w.on("assist");
        let fake = Fake::answering(vec![says(&format!("p{}", f.api), 0.9)]);
        f.preview(&fake, Some(f.app))
            .await
            .suggested_sibling
            .unwrap();
        let args = StartArgs {
            project_id: None,
            decider,
            ..f.args(None)
        };
        let store = Arc::clone(&f.w.store);
        tickets::start_many(
            &f.w.store,
            &args,
            &[f.app, f.api],
            &ViewScope::internal(),
            &crate::service::trackers::TrackerNet::fake(Arc::new(
                crate::net::https::FakeTransport::new(),
            )),
            tokio::time::Instant::now() + Duration::from_secs(30),
            move |a| {
                let s = store.lock().unwrap();
                let name = a.new_worktree.clone().unwrap_or_else(|| "x".into());
                let id = s
                    .upsert_session(&name, &a.host_alias, None, None, 1, 1, "running", None)
                    .unwrap();
                std::future::ready(Ok(s.get_session_by_id(id).unwrap().unwrap()))
            },
            |_, _| {},
        )
        .await
        .unwrap();
        let runs = f.w.runs();
        let r = runs
            .iter()
            .find(|r| r.feature == "sibling_repos")
            .expect("the run");
        assert_eq!(r.followup.as_deref(), want, "{decider:?}");
    }
}
