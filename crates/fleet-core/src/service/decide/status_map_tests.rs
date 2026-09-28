//! J3 `status_map` over a scripted backend: the gate, shadow and assist,
//! fallbacks, the re-ask rule, the bound, follow-ups, the proposals view, and
//! no section name in the record. No test reaches TypeSafe.

use super::*;
use crate::service::decide::{BackendError, DecisionBackend, JevResponse, PROVIDER_JEV};
use crate::service::trackers::admin::{admin_sync, WorkAdminArgs};
use crate::store::TrackerConfig;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicUsize};
use std::time::Duration;

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

/// A choice answer with a two-option distribution.
fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == "unsure" { "todo" } else { "unsure" };
    let mut probabilities = serde_json::Map::new();
    if OPTIONS.iter().any(|(k, _)| *k == choice) {
        probabilities.insert(choice.into(), json!(confidence));
        probabilities.insert(other.into(), json!(1.0 - confidence));
    }
    Ok(serde_json::from_value(json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": probabilities,
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 120, "output_tokens": 3 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    org: i64,
    tracker: i64,
    clock: Arc<AtomicI64>,
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// An Asana tracker of org Acme, probed: two sections the rule mapped, and
/// three it left to `todo`.
fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    let t = s
        .add_tracker("asana", "Company B", "https://app.asana.com")
        .unwrap()
        .id;
    s.set_tracker_org(t, Some(org)).unwrap();
    let cfg = TrackerConfig {
        workspace: Some("1200000000000001".into()),
        section_map: [("in progress", "in_progress"), ("done", "done")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        unmapped_sections: names(&["backlog", "ideas", "parked"]),
        project_sections: vec![(
            "1200000000001001".into(),
            names(&["ideas", "backlog", "in progress", "done", "parked"]),
        )],
        ..Default::default()
    };
    s.set_tracker_probe(t, Some("1200000000000001"), &cfg)
        .unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        org,
        tracker: t,
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
    /// The kill switch on, `status_map` in `mode`, a key, the org consenting.
    fn on(&self, mode: &str) {
        self.set(settings::DECIDE_JEV_ENABLED, "true");
        self.set(settings::DECIDE_JEV_STATUS_MAP, mode);
        let s = self.store.lock().unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&crate::store::DecisionRunFilter::default())
            .unwrap()
    }
    fn proposals(&self) -> TrackerProposals {
        let mut all = proposals(&self.store.lock().unwrap(), Some(self.tracker)).unwrap();
        assert_eq!(all.len(), 1);
        all.remove(0)
    }
    async fn run(&self, fake: &Arc<Fake>) -> RunReport {
        propose_for_tracker(&self.ctx(fake.clone()), self.tracker)
            .await
            .unwrap()
    }
    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
}

// --- the gate -------------------------------------------------------------------

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("todo", 0.9)]);
    let r = w.run(&fake).await;
    assert_eq!(r.gated, Some(Fallback::FlagOff));
    assert_eq!((r.asked, fake.calls()), (0, 0));
    assert!(w.runs().is_empty(), "a refused gate records no row");
}

#[tokio::test]
async fn an_org_that_did_not_consent_is_never_sent_anything() {
    let w = world();
    w.on("assist");
    w.store
        .lock()
        .unwrap()
        .set_org_jev_allowed(w.org, false)
        .unwrap();
    let fake = Fake::answering(vec![says("todo", 0.9)]);
    let r = w.run(&fake).await;
    assert_eq!(r.gated, Some(Fallback::OrgOff));
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
    // A tracker with no org follows decide.jev.unassigned (off).
    w.store
        .lock()
        .unwrap()
        .set_tracker_org(w.tracker, None)
        .unwrap();
    assert_eq!(w.run(&fake).await.gated, Some(Fallback::OrgOff));
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
async fn the_mode_off_asks_nothing() {
    let w = world();
    w.on("off");
    let fake = Fake::answering(vec![]);
    assert_eq!(w.run(&fake).await.gated, Some(Fallback::ModeOff));
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn a_tracker_that_is_not_asana_is_never_looked_at() {
    let w = world();
    w.on("shadow");
    let jira = w
        .store
        .lock()
        .unwrap()
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap()
        .id;
    let fake = Fake::answering(vec![]);
    let r = propose_for_tracker(&w.ctx(fake.clone()), jira)
        .await
        .unwrap();
    assert_eq!((r.asked, r.gated, fake.calls()), (0, None, 0));
    assert!(w.runs().is_empty());
}

// --- shadow ---------------------------------------------------------------------

#[tokio::test]
async fn shadow_asks_every_section_and_records_the_rule_as_baseline() {
    let w = world();
    w.on("shadow");
    // Unmapped first (backlog, ideas, parked), then the rule's (done, in
    // progress).
    let fake = Fake::answering(vec![
        says("todo", 0.9),
        says("todo", 0.8),
        says("not_planned", 0.7),
        says("done", 0.95),
        says("done", 0.6),
    ]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.usable, r.mode), (5, 5, Some(Mode::Shadow)));
    let runs = w.runs();
    assert_eq!(runs.len(), 5);
    let mut baselines: Vec<&str> = runs
        .iter()
        .map(|r| r.baseline_answer.as_deref().unwrap())
        .collect();
    baselines.sort();
    assert_eq!(
        baselines,
        vec!["done", "in_progress", "none", "none", "none"]
    );
    for r in &runs {
        assert_eq!(r.subject_kind, SUBJECT_KIND);
        assert!(r.subject_id.starts_with(&format!("{}:", w.tracker)));
        assert_eq!(r.question_version, QUESTION_VERSION);
        assert_eq!(r.org_id, Some(w.org));
        assert_eq!(
            r.candidates,
            names(&["done", "in_progress", "not_planned", "todo", "unsure"])
        );
    }
    let p = w.proposals();
    assert!(p.proposals.is_empty(), "shadow proposes nothing");
    assert!(p.apply.is_none());
    assert_eq!(p.shadow.len(), 5);
    assert_eq!(
        p.shadow_agreement(),
        (2, 1),
        "done agreed, in progress did not"
    );
    let differ = p
        .shadow
        .iter()
        .find(|l| l.section == "in progress")
        .unwrap();
    assert_eq!(
        (differ.rule.as_str(), differ.model.as_str()),
        ("in_progress", "done")
    );
    let text = p.lines().join("\n");
    assert!(text.contains("agree on 1 of 2"), "{text}");
    assert!(text.contains("DIFFER"), "{text}");
    // The envelope's own stats see the comparison too.
    let stats = w.store.lock().unwrap().decision_stats(0).unwrap();
    let answered = stats.iter().find(|s| s.fallback.is_none()).unwrap();
    assert_eq!((answered.compared, answered.agreed), (5, 1));
}

#[tokio::test]
async fn what_is_sent_is_the_section_its_board_and_the_provider_only() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("todo", 0.9),
        says("todo", 0.9),
        says("done", 0.9),
    ]);
    w.run(&fake).await;
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "assist asks the unmapped sections only");
    let first = &seen[0];
    assert_eq!(
        first.state,
        json!({
            "provider": "asana",
            "section": "backlog",
            "project_sections": ["ideas", "backlog", "in progress", "done", "parked"],
        })
    );
    assert_eq!(first.question, question());
    first.question.check().unwrap();
}

// --- assist ---------------------------------------------------------------------

#[tokio::test]
async fn assist_proposals_are_listed_with_the_command_that_applies_them() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("todo", 0.91),
        says("unsure", 0.8),
        says("not_planned", 0.85),
    ]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.usable), (3, 3));
    // Nothing is written to the tracker: proposals only.
    let row = w.store.lock().unwrap().require_tracker(w.tracker).unwrap();
    assert!(row.settings.section_map.is_empty() && !row.settings.section_map_confirmed);
    assert_eq!(row.config.section_map.len(), 2);

    let p = w.proposals();
    assert_eq!(p.mode, "assist");
    let got: Vec<(&str, &str, Option<&str>)> = p
        .proposals
        .iter()
        .map(|p| {
            (
                p.section.as_str(),
                p.answer.as_str(),
                p.applies_as.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("backlog", "todo", Some("todo")),
            ("ideas", "unsure", None),
            ("parked", "not_planned", Some("done")),
        ]
    );
    let apply = p.apply.as_ref().unwrap();
    assert_eq!(
        apply.cli,
        format!(
            "fleet-hub tracker section-map {} --set 'backlog=todo' --set 'parked=done'",
            w.tracker
        )
    );
    assert_eq!(
        apply.work_admin,
        json!({
            "action": "update",
            "tracker_id": w.tracker,
            "settings": {
                "section_map": {
                    "backlog": "todo",
                    "done": "done",
                    "in progress": "in_progress",
                    "parked": "done",
                },
                "section_map_confirmed": true,
            },
        })
    );
    let text = p.lines().join("\n");
    assert!(text.contains("\"backlog\" → todo (0.91)"), "{text}");
    assert!(text.contains("applies as done"), "{text}");
    assert!(text.contains("proposes nothing"), "{text}");
    // The work_admin arguments are exactly what update accepts.
    let args: WorkAdminArgs = serde_json::from_value(apply.work_admin.clone()).unwrap();
    admin_sync(&args, &w.store).unwrap();
}

#[tokio::test]
async fn an_answer_that_does_not_fit_or_is_unsure_of_itself_is_not_proposed() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("blocked", 0.9),
        says("todo", 0.3),
        Err(BackendError::Timeout),
    ]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.usable), (3, 0));
    let mut fallbacks: Vec<String> = w.runs().into_iter().filter_map(|r| r.fallback).collect();
    fallbacks.sort();
    assert_eq!(
        fallbacks,
        vec!["invalid_answer", "low_confidence", "timeout"]
    );
    let p = w.proposals();
    assert!(p.proposals.is_empty() && p.apply.is_none());
}

#[tokio::test]
async fn a_rate_limit_stops_the_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![Err(BackendError::RateLimited {
        retry_after: Some(30),
    })]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.stopped), (1, Some(Fallback::RateLimited)));
    assert_eq!(fake.calls(), 1);
    assert_eq!(w.runs().len(), 1);
}

// --- bounds -----------------------------------------------------------------------

#[tokio::test]
async fn a_section_decided_recently_on_the_same_input_is_not_asked_again() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("todo", 0.9),
        says("todo", 0.9),
        Err(BackendError::Timeout),
    ]);
    w.run(&fake).await;
    assert_eq!(fake.calls(), 3);
    // Next day: the two decided are skipped; the timed-out one is asked.
    w.advance(86_400);
    let fake = Fake::answering(vec![says("done", 0.9)]);
    let r = w.run(&fake).await;
    assert_eq!((r.skipped_recent, r.asked), (2, 1));
    // A new board order is a new input: asked again.
    {
        let s = w.store.lock().unwrap();
        let mut cfg = s.require_tracker(w.tracker).unwrap().config;
        cfg.project_sections[0].1.push("archive".into());
        s.set_tracker_probe(w.tracker, None, &cfg).unwrap();
    }
    w.advance(60);
    let fake = Fake::answering(vec![says("todo", 0.9); 3]);
    assert_eq!(w.run(&fake).await.asked, 3);
    // After REASK_DAYS, everything is asked again.
    w.advance(REASK_DAYS * 86_400 + 1);
    let fake = Fake::answering(vec![says("todo", 0.9); 3]);
    assert_eq!(w.run(&fake).await.asked, 3);
    // Shadow is another mode: its own answers.
    w.set(settings::DECIDE_JEV_STATUS_MAP, "shadow");
    let fake = Fake::answering(vec![says("todo", 0.9); 5]);
    assert_eq!(w.run(&fake).await.asked, 5);
}

#[tokio::test]
async fn a_run_asks_at_most_its_cap() {
    let w = world();
    w.on("assist");
    {
        let s = w.store.lock().unwrap();
        let mut cfg = s.require_tracker(w.tracker).unwrap().config;
        cfg.unmapped_sections = (0..MAX_ASKS_PER_RUN + 10)
            .map(|i| format!("stage {i}"))
            .collect();
        s.set_tracker_probe(w.tracker, None, &cfg).unwrap();
    }
    let fake = Fake::answering(vec![says("todo", 0.9); MAX_ASKS_PER_RUN + 10]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.deferred), (MAX_ASKS_PER_RUN, 10));
    assert_eq!(fake.calls(), MAX_ASKS_PER_RUN);
    // The next run takes the rest.
    w.advance(3_600);
    let r = w.run(&fake).await;
    assert_eq!(
        (r.skipped_recent, r.asked, r.deferred),
        (MAX_ASKS_PER_RUN, 10, 0)
    );
}

// --- follow-up ------------------------------------------------------------------

#[tokio::test]
async fn a_persons_map_confirms_or_corrects_the_proposals() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![
        says("todo", 0.9),
        says("unsure", 0.9),
        says("not_planned", 0.9),
    ]);
    w.run(&fake).await;
    // They keep backlog as todo, put parked in progress, and ideas (the
    // model was unsure) as todo.
    let args: WorkAdminArgs = serde_json::from_value(json!({
        "action": "update",
        "tracker_id": w.tracker,
        "settings": {
            "section_map": { "backlog": "todo", "parked": "in_progress", "ideas": "todo" },
            "section_map_confirmed": true,
        },
    }))
    .unwrap();
    admin_sync(&args, &w.store).unwrap();
    let p = w.proposals();
    let by = |n: &str| p.proposals.iter().find(|x| x.section == n).unwrap().clone();
    assert_eq!(by("backlog").followup.as_deref(), Some("confirmed"));
    assert_eq!(by("parked").followup.as_deref(), Some("corrected"));
    assert_eq!(
        by("ideas").followup,
        None,
        "unsure proposed nothing to follow up"
    );
    let parked = w
        .runs()
        .into_iter()
        .find(|r| r.id == by("parked").run_id)
        .unwrap();
    assert_eq!(parked.corrected_to.as_deref(), Some("in_progress"));
    assert!(p.apply.is_none(), "nothing pending once the person decided");
    // Again: a follow-up is recorded once.
    admin_sync(&args, &w.store).unwrap();
    {
        let s = w.store.lock().unwrap();
        let row = s.require_tracker(w.tracker).unwrap();
        assert_eq!(record_followups(&s, &row, NOON).unwrap(), 0);
    }
    // And the person's sections are theirs: never asked again.
    w.advance(REASK_DAYS * 86_400 + 1);
    let fake = Fake::answering(vec![]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, fake.calls()), (0, 0));
}

// --- the record -------------------------------------------------------------------

#[tokio::test]
async fn no_section_name_is_ever_in_the_record() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("todo", 0.9); 5]);
    w.run(&fake).await;
    let dump = serde_json::to_string(&w.runs()).unwrap();
    for name in ["backlog", "ideas", "parked", "in progress"] {
        assert!(!dump.contains(name), "{name:?} leaked into {dump}");
    }
    // The ids are stable across processes that share the key, and not
    // the name.
    let s = w.store.lock().unwrap();
    let key = s.decision_fp_key().unwrap();
    let id = section_id(&key, w.tracker, "backlog");
    assert_eq!(id.len(), 16);
    assert_eq!(id, section_id(&key, w.tracker, "backlog"));
    assert_ne!(id, section_id(&key, w.tracker + 1, "backlog"));
    assert_ne!(id, section_id(&Secret::new("other"), w.tracker, "backlog"));
}

#[tokio::test]
async fn proposals_read_a_read_only_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let w = world();
    let tracker;
    {
        let s = Store::open_with_bus(&path, Arc::new(crate::events::NoopEventBus)).unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        tracker = s
            .add_tracker("asana", "B", "https://app.asana.com")
            .unwrap()
            .id;
        s.set_tracker_org(tracker, Some(org)).unwrap();
        let cfg = w
            .store
            .lock()
            .unwrap()
            .require_tracker(w.tracker)
            .unwrap()
            .config;
        s.set_tracker_probe(tracker, None, &cfg).unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_STATUS_MAP, "assist").unwrap();
        s.set_org_jev_allowed(org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
        let store = Arc::new(Mutex::new(s));
        let fake = Fake::answering(vec![says("todo", 0.9); 3]);
        let ctx = DecideCtx::new(store, fake).with_clock(Arc::new(|| NOON));
        assert_eq!(propose_for_tracker(&ctx, tracker).await.unwrap().asked, 3);
    }
    let ro = Store::open_read_only(&path).unwrap();
    let p = proposals(&ro, None).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].proposals.len(), 3);
    assert!(p[0].apply.is_some());
}

// --- a person decides one proposal ------------------------------------------------

impl World {
    /// Assist, one run: backlog → todo, ideas → unsure, parked →
    /// not_planned. Returns the run id per section.
    async fn three_proposals(&self) -> HashMap<String, i64> {
        self.on("assist");
        let fake = Fake::answering(vec![
            says("todo", 0.91),
            says("unsure", 0.8),
            says("not_planned", 0.85),
        ]);
        self.run(&fake).await;
        self.proposals()
            .proposals
            .into_iter()
            .map(|p| (p.section, p.run_id))
            .collect()
    }
    fn decide(&self, run: i64, action: ProposalAction) -> Result<ProposalOutcome, IpcError> {
        decide_proposal(&self.store, run, &action, NOON)
    }
    fn row(&self) -> TrackerRow {
        self.store
            .lock()
            .unwrap()
            .require_tracker(self.tracker)
            .unwrap()
    }
    fn run_row(&self, id: i64) -> DecisionRunRow {
        self.store
            .lock()
            .unwrap()
            .get_decision_run(id)
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn apply_puts_the_category_in_the_persons_map_and_confirms_the_run() {
    let w = world();
    let runs = w.three_proposals().await;
    // The why: the two most probable options, most probable first.
    let p = w.proposals();
    let backlog = p.proposals.iter().find(|x| x.section == "backlog").unwrap();
    let top: Vec<(&str, String)> = backlog
        .top
        .iter()
        .map(|(k, v)| (k.as_str(), format!("{v:.2}")))
        .collect();
    assert_eq!(
        top,
        vec![("todo", "0.91".into()), ("unsure", "0.09".into())]
    );
    assert!(backlog.pending());

    let out = w.decide(runs["backlog"], ProposalAction::Apply).unwrap();
    assert_eq!(
        (
            out.section.as_str(),
            out.category.as_deref(),
            out.followup.as_deref()
        ),
        ("backlog", Some("todo"), Some("confirmed"))
    );
    // Through work_admin update: the map is confirmed, the inferred one
    // kept under the person's entry, the other proposals untouched.
    let row = w.row();
    assert!(row.settings.section_map_confirmed);
    assert_eq!(
        row.settings.section_map,
        [
            ("backlog", "todo"),
            ("done", "done"),
            ("in progress", "in_progress")
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    );
    assert!(w.run_row(runs["parked"]).followup.is_none());
    // not_planned applies as done.
    let out = w.decide(runs["parked"], ProposalAction::Apply).unwrap();
    assert_eq!(out.category.as_deref(), Some("done"));
    assert_eq!(out.followup.as_deref(), Some("confirmed"));
    assert_eq!(w.row().settings.section_map["parked"], "done");
    // Decided: not pending, and not decidable twice.
    let p = w.proposals();
    assert!(p.proposals.iter().filter(|x| x.pending()).count() == 1);
    let e = w
        .decide(runs["backlog"], ProposalAction::Apply)
        .unwrap_err();
    assert!(e.message.contains("already in your section map"), "{e:?}");
}

#[tokio::test]
async fn apply_as_is_a_correction_and_unsure_can_only_be_applied_as() {
    let w = world();
    let runs = w.three_proposals().await;
    let e = w.decide(runs["ideas"], ProposalAction::Apply).unwrap_err();
    assert!(e.message.contains("proposes nothing"), "{e:?}");
    let out = w
        .decide(runs["ideas"], ProposalAction::ApplyAs("in_progress".into()))
        .unwrap();
    assert_eq!(
        (out.followup.as_deref(), out.corrected_to.as_deref()),
        (Some("corrected"), Some("in_progress"))
    );
    assert_eq!(w.row().settings.section_map["ideas"], "in_progress");
    let out = w
        .decide(runs["backlog"], ProposalAction::ApplyAs("done".into()))
        .unwrap();
    assert_eq!(out.followup.as_deref(), Some("corrected"));
    assert_eq!(
        w.run_row(runs["backlog"]).corrected_to.as_deref(),
        Some("done")
    );
    // apply_as the answer's own category is a confirmation.
    let out = w
        .decide(runs["parked"], ProposalAction::ApplyAs("done".into()))
        .unwrap();
    assert_eq!(
        (out.followup.as_deref(), out.corrected_to),
        (Some("confirmed"), None)
    );
}

#[tokio::test]
async fn reject_leaves_the_section_unmapped_and_hides_the_answer_until_a_new_one() {
    let w = world();
    let runs = w.three_proposals().await;
    let out = w.decide(runs["backlog"], ProposalAction::Reject).unwrap();
    assert_eq!(
        (out.category, out.followup.as_deref()),
        (None, Some("rejected"))
    );
    let row = w.row();
    assert!(row.settings.section_map.is_empty() && !row.settings.section_map_confirmed);
    let p = w.proposals();
    assert_eq!(p.rejected, 1);
    assert!(!p.proposals.iter().any(|x| x.section == "backlog"));
    assert!(!p.apply.as_ref().unwrap().cli.contains("backlog"));
    assert!(p
        .lines()
        .join("\n")
        .contains("1 rejected proposal(s) hidden"));
    let e = w
        .decide(runs["backlog"], ProposalAction::Apply)
        .unwrap_err();
    assert!(e.message.contains("already rejected"), "{e:?}");

    // After the re-ask window, the same input and pinned model are not
    // asked again: the answer would be the same one.
    w.advance(REASK_DAYS * 86_400 + 1);
    let fake = Fake::answering(vec![says("unsure", 0.8), says("not_planned", 0.85)]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.skipped_rejected), (2, 1));
    assert_eq!(w.proposals().rejected, 1);
    // A new input (the board changed) is a new answer: proposed again.
    {
        let s = w.store.lock().unwrap();
        let mut cfg = s.require_tracker(w.tracker).unwrap().config;
        cfg.project_sections[0].1.push("archive".into());
        s.set_tracker_probe(w.tracker, None, &cfg).unwrap();
    }
    let fake = Fake::answering(vec![says("todo", 0.9); 3]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.skipped_rejected), (3, 0));
    let p = w.proposals();
    assert_eq!(p.rejected, 0);
    let again = p.proposals.iter().find(|x| x.section == "backlog").unwrap();
    assert!(again.pending() && again.run_id != runs["backlog"]);
}

#[tokio::test]
async fn only_the_latest_usable_assist_answer_of_a_listed_section_is_decided() {
    let w = world();
    // Not a run at all, and a shadow run.
    assert_eq!(
        w.decide(9_999, ProposalAction::Reject).unwrap_err().code,
        crate::ipc_error::codes::E_NOTFOUND
    );
    w.on("shadow");
    w.run(&Fake::answering(vec![says("todo", 0.9); 5])).await;
    let shadow = w.runs()[0].id;
    let e = w.decide(shadow, ProposalAction::Reject).unwrap_err();
    assert!(e.message.contains("not a usable assist answer"), "{e:?}");
    // A low-confidence assist answer.
    w.on("assist");
    w.advance(60);
    w.run(&Fake::answering(vec![
        says("todo", 0.3),
        says("todo", 0.9),
        says("done", 0.9),
    ]))
    .await;
    let low = w
        .runs()
        .into_iter()
        .find(|r| r.fallback.as_deref() == Some("low_confidence"))
        .unwrap()
        .id;
    assert!(w.decide(low, ProposalAction::Apply).is_err());
    // Another feature's run.
    let other = w
        .store
        .lock()
        .unwrap()
        .insert_decision_run(&crate::store::NewDecisionRun {
            at: NOON,
            feature: "work_link".into(),
            subject_kind: "session".into(),
            subject_id: format!("{}:x", w.tracker),
            mode: "assist".into(),
            provider: "jev".into(),
            question_version: "wl.1".into(),
            answer: Some("todo".into()),
            called: true,
            ..Default::default()
        })
        .unwrap();
    let e = w.decide(other, ProposalAction::Apply).unwrap_err();
    assert!(e.message.contains("not a status_map proposal"), "{e:?}");
    // An older answer, once a newer one exists for the section.
    let ideas_old = w
        .proposals()
        .proposals
        .iter()
        .find(|p| p.section == "ideas")
        .unwrap()
        .run_id;
    w.advance(REASK_DAYS * 86_400 + 1);
    w.run(&Fake::answering(vec![says("todo", 0.9); 3])).await;
    let e = w.decide(ideas_old, ProposalAction::Apply).unwrap_err();
    assert!(e.message.contains("not the latest proposal"), "{e:?}");
    // A section the tracker no longer lists.
    let ideas = w
        .proposals()
        .proposals
        .iter()
        .find(|p| p.section == "ideas")
        .unwrap()
        .run_id;
    {
        let s = w.store.lock().unwrap();
        let mut cfg = s.require_tracker(w.tracker).unwrap().config;
        cfg.unmapped_sections.retain(|n| n != "ideas");
        s.set_tracker_probe(w.tracker, None, &cfg).unwrap();
    }
    let e = w.decide(ideas, ProposalAction::Apply).unwrap_err();
    assert!(e.message.contains("no longer lists"), "{e:?}");
    // A category outside the section map's vocabulary.
    assert!(ProposalAction::parse("apply_as", Some("not_planned")).is_err());
    assert!(ProposalAction::parse("apply_as", None).is_err());
    assert!(ProposalAction::parse("apply", Some("done")).is_err());
    assert!(ProposalAction::parse("maybe", None).is_err());
    assert_eq!(
        ProposalAction::parse("apply_as", Some(" done ")).unwrap(),
        ProposalAction::ApplyAs("done".into())
    );
    // Nothing above wrote the tracker.
    assert!(w.row().settings.section_map.is_empty());
}

// --- pure parts and the trigger ---------------------------------------------------

#[test]
fn the_board_is_a_window_around_the_section() {
    let w = world();
    let mut row = w.store.lock().unwrap().require_tracker(w.tracker).unwrap();
    assert_eq!(board_of(&row, "parked").len(), 5);
    assert!(board_of(&row, "nowhere").is_empty());
    let long: Vec<String> = (0..80).map(|i| format!("s{i}")).collect();
    row.config.project_sections = vec![("1".into(), long)];
    let b = board_of(&row, "s60");
    assert_eq!(b.len(), MAX_PROJECT_SECTIONS);
    assert!(b.contains(&"s60".to_string()));
    let b = board_of(&row, "s79");
    assert_eq!(b.last().map(String::as_str), Some("s79"));
    let b = board_of(&row, "s0");
    assert_eq!(b.first().map(String::as_str), Some("s0"));
    // The request the adapter sends (and the benchmark reuses): the board
    // windowed once, the same whether or not it was windowed before.
    let full = &row.config.project_sections[0].1;
    let req = question_for("s60", full);
    assert_eq!(req, question_for("s60", &board_of(&row, "s60")));
    assert_eq!(req.state, state("s60", &board_of(&row, "s60")));
    assert_eq!(req.question, question());
    assert_eq!(
        board_window(full, "missing").first().map(String::as_str),
        Some("s0")
    );
}

#[test]
fn the_question_is_a_valid_choice_over_the_categories() {
    let q = question();
    q.check().unwrap();
    assert_eq!(
        q.candidates(),
        names(&["done", "in_progress", "not_planned", "todo", "unsure"])
    );
    assert_eq!(applied_category("not_planned"), Some("done"));
    assert_eq!(applied_category("unsure"), None);
    assert!(due(None, NOON, 1));
    assert!(!due(Some((NOON, 1)), NOON + 60, 1));
    assert!(due(Some((NOON, 1)), NOON + 60, 2), "sections changed");
    assert!(due(Some((NOON, 1)), NOON + RUN_EVERY_SECS, 1));
}

#[tokio::test]
async fn the_sync_hook_runs_a_clean_asana_pass_once_a_day() {
    let w = world();
    let fake = Fake::answering(vec![says("todo", 0.9); 3]);
    let trigger = StatusMapTrigger::new(w.ctx(fake.clone()));
    let pass = |error: Option<&str>| TrackerPass {
        tracker_id: w.tracker,
        error: error.map(str::to_string),
        ..Default::default()
    };
    // Off by default: not even a task.
    assert!(trigger.after_pass(&[pass(None)]).is_none());
    w.on("assist");
    assert!(trigger.after_pass(&[pass(Some("offline"))]).is_none());
    trigger.after_pass(&[pass(None)]).unwrap().await.unwrap();
    assert_eq!(fake.calls(), 3);
    assert!(trigger.after_pass(&[pass(None)]).is_none(), "once a day");
    w.advance(RUN_EVERY_SECS);
    trigger.after_pass(&[pass(None)]).unwrap().await.unwrap();
    assert_eq!(fake.calls(), 3, "due again, but nothing new to ask");
}

#[tokio::test]
async fn a_tracker_due_while_a_run_is_going_stays_due() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("todo", 0.9); 3]);
    let trigger = StatusMapTrigger::new(w.ctx(fake.clone()));
    let pass = TrackerPass {
        tracker_id: w.tracker,
        ..Default::default()
    };
    // Another run holds the single flight: nothing starts, and nothing is
    // marked as run.
    trigger.running.store(true, Ordering::SeqCst);
    assert!(trigger.after_pass(std::slice::from_ref(&pass)).is_none());
    assert!(trigger.last.lock().unwrap().is_empty());
    // It ends: the next pass runs the tracker, not a day later.
    trigger.running.store(false, Ordering::SeqCst);
    trigger
        .after_pass(std::slice::from_ref(&pass))
        .unwrap()
        .await
        .unwrap();
    assert_eq!(fake.calls(), 3);
}

#[tokio::test]
async fn a_proposal_nobody_decided_is_ignored_once_a_newer_one_arrives() {
    let w = world();
    let runs = w.three_proposals().await;
    w.decide(runs["backlog"], ProposalAction::Reject).unwrap();
    // After the re-ask window: ideas gets a proposal under the floor (not
    // one that is shown), parked a new usable one.
    w.advance(REASK_DAYS * 86_400 + 1);
    let fake = Fake::answering(vec![says("todo", 0.3), says("done", 0.9)]);
    let r = w.run(&fake).await;
    assert_eq!((r.asked, r.skipped_rejected), (2, 1));
    // The superseded proposal is `ignored`; the one still shown is not;
    // a person's rejection is never overwritten.
    assert_eq!(
        w.run_row(runs["parked"]).followup.as_deref(),
        Some("ignored")
    );
    assert!(w.run_row(runs["ideas"]).followup.is_none());
    assert_eq!(
        w.run_row(runs["backlog"]).followup.as_deref(),
        Some("rejected")
    );
    let p = w.proposals();
    let parked = p.proposals.iter().find(|x| x.section == "parked").unwrap();
    assert!(parked.pending() && parked.run_id != runs["parked"]);
    let stats = w.store.lock().unwrap().decision_stats(0).unwrap();
    assert_eq!(stats.iter().map(|s| s.ignored).sum::<i64>(), 1);
}
