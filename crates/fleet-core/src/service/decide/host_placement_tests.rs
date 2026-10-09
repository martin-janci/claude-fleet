//! N5 `host_placement` (redesign 4.11) over a scripted backend: the
//! numbers first (offline, hidden, other org, over the limit), then the
//! gate, assist and shadow, reuse, follow-ups. No test reaches TypeSafe.

use super::host_placement::*;
use super::*;
use crate::service::account_usage::{
    AccountUsage, AccountUsageSnapshot, UsageCache, UsageOutcomeKind, Window,
};
use crate::store::{DecisionRunFilter, DecisionRunRow, HostRow, OrgRuleRow};
use std::collections::{BTreeMap, VecDeque};
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
    let other = if choice == UNSURE { "h:local" } else { UNSURE };
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

fn host(alias: &str, reachable: bool, account: Option<&str>) -> HostRow {
    serde_json::from_value(serde_json::json!({
        "alias": alias, "ssh_alias": null, "reachable": reachable, "claude_version": null,
        "tmux_version": null, "hidden": false, "last_pinged_at": 1,
        "account_uuid": account, "provisioned": true, "transport": "ssh",
    }))
    .unwrap()
}

fn snap(uuid: &str, used: f64) -> AccountUsageSnapshot {
    AccountUsageSnapshot {
        account_uuid: uuid.into(),
        usage: Some(AccountUsage {
            five_hour: Some(Window {
                utilization: used,
                resets_at: Some(NOON + 3_600),
            }),
            seven_day: None,
            seven_day_opus: None,
            seven_day_sonnet: None,
        }),
        subscription: None,
        fetched_at: Some(NOON - 60),
        source_host: None,
        status: UsageOutcomeKind::Ok,
        detail: None,
        next_try_at: 0,
    }
}

fn aliases(c: &[Candidate]) -> Vec<&str> {
    c.iter().map(|c| c.alias.as_str()).collect()
}

// --- the numbers ---------------------------------------------------------------

/// The plan's check for 4.11: a host over its limit or offline is never a
/// candidate, so it is never proposed.
#[test]
fn a_host_over_its_limit_offline_or_hidden_is_never_a_candidate() {
    let mut hidden = host("attic", true, None);
    hidden.hidden = true;
    let hosts = vec![
        host("mercury", true, Some("busy")),
        host("venus", false, None),
        hidden,
        host("mars", true, Some("calm")),
        host("local", false, Some("calm")),
    ];
    let usage = [snap("busy", 95.0), snap("calm", 37.0)];
    let c = candidates(&hosts, &usage, 90.0, &BTreeMap::new(), None, |_| None, NOON);
    assert_eq!(
        aliases(&c),
        ["local", "mars"],
        "local is always up and comes first; mercury is past the line, venus offline, attic hidden"
    );
    assert_eq!(c[1].account_used_pct, Some(30), "bucketed to a tenth");
}

#[test]
fn inside_a_project_org_only_hosts_that_keep_the_session_there_count() {
    let hosts = vec![
        host("local", true, None),
        host("mars", true, None),
        host("pluto", true, None),
    ];
    let orgs = BTreeMap::from([("local", Some(1)), ("mars", Some(1)), ("pluto", Some(2))]);
    let c = candidates(
        &hosts,
        &[],
        90.0,
        &BTreeMap::new(),
        Some(1),
        |a| orgs.get(a).copied().flatten(),
        NOON,
    );
    assert_eq!(aliases(&c), ["local", "mars"]);
    let all = candidates(&hosts, &[], 90.0, &BTreeMap::new(), None, |_| Some(9), NOON);
    assert_eq!(all.len(), 3, "a project with no org may go anywhere");
}

#[test]
fn the_counts_and_probe_numbers_are_bucketed() {
    let mut mars = host("mars", true, None);
    mars.disk_home_free_kb = Some(379);
    mars.disk_home_total_kb = Some(1000);
    mars.latency_ms = Some(74);
    let counts = BTreeMap::from([("mars".to_string(), (3, 5, true))]);
    let c = candidates(&[mars], &[], 90.0, &counts, None, |_| None, NOON);
    assert_eq!(
        c[0],
        Candidate {
            alias: "mars".into(),
            recent_starts: 5,
            checked_out: true,
            live_sessions: 3,
            disk_free_pct: Some(30),
            latency_ms: Some(50),
            account_used_pct: None,
        }
    );
}

// --- the store -----------------------------------------------------------------

struct World {
    store: Arc<Mutex<Store>>,
    cache: Mutex<UsageCache>,
    org: i64,
    project: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    let project = s.upsert_project("acme", "pos", "/p/acme/pos").unwrap();
    s.add_org_rule(OrgRuleRow {
        id: 0,
        org_id: org,
        owner: Some("acme".into()),
        repo: None,
        host_alias: None,
        path_prefix: None,
    })
    .unwrap();
    s.upsert_host("local").unwrap();
    for (alias, up) in [("mars", true), ("venus", false)] {
        s.insert_host(alias, None).unwrap();
        s.update_host_probe(alias, up, None, None, NOON).unwrap();
    }
    s.upsert_worktree_on("mars", project, "pos", "/home/m/pos", Some("main"))
        .unwrap();
    s.upsert_session(
        "pos-1",
        "mars",
        Some(project),
        None,
        NOON - 3_600,
        NOON,
        "idle",
        None,
    )
    .unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        cache: Mutex::new(UsageCache::new()),
        org,
        project,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone()).with_clock(Arc::new(|| NOON))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_HOST_PLACEMENT, mode).unwrap();
        s.set_org_jev_allowed(self.org, true).unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn args(&self) -> ProposeHostArgs {
        ProposeHostArgs {
            project_id: self.project,
        }
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
}

#[test]
fn the_input_reads_the_projects_org_hosts_and_counts() {
    let w = world();
    let input = input_for(&w.store, &w.cache, w.project, NOON)
        .unwrap()
        .unwrap();
    assert_eq!(input.org_id, Some(w.org), "the owner rule");
    assert_eq!(
        aliases(&input.candidates),
        ["local", "mars"],
        "venus is offline"
    );
    let mars = &input.candidates[1];
    assert_eq!(
        (mars.recent_starts, mars.live_sessions, mars.checked_out),
        (1, 1, true)
    );
    assert!(input_for(&w.store, &w.cache, 999, NOON).unwrap().is_none());
}

#[tokio::test]
async fn with_the_defaults_nothing_is_asked_and_nothing_recorded() {
    let w = world();
    let fake = Fake::answering(vec![says("h:mars", 0.9)]);
    assert_eq!(
        propose(&w.ctx(&fake), &w.cache, &w.args()).await.unwrap(),
        None
    );
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn assist_proposes_a_candidate_and_reuses_the_decided_run() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("h:mars", 0.8)]);
    let ctx = w.ctx(&fake);
    let got = propose(&ctx, &w.cache, &w.args())
        .await
        .unwrap()
        .expect("a host");
    assert_eq!(
        (got.host_alias.as_str(), got.confidence_pct),
        ("mars", Some(80))
    );
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].subject_id, format!("project:{}", w.project));
    assert_eq!(runs[0].baseline_answer.as_deref(), Some("h:local"));
    // The dialog opened again on the same project: no second call.
    let again = propose(&ctx, &w.cache, &w.args())
        .await
        .unwrap()
        .expect("reused");
    assert_eq!(again.host_alias, "mars");
    assert_eq!(fake.calls(), 1);
    // What was sent: the project's name and the candidates' numbers.
    let sent = fake.seen.lock().unwrap()[0].clone();
    assert_eq!(sent.state["project"], "acme/pos");
    assert_eq!(sent.state["hosts"]["h:mars"]["checked_out"], true);
    let Question::Choice { criteria, .. } = sent.question else {
        panic!("a choice");
    };
    assert_eq!(
        criteria.keys().cloned().collect::<Vec<_>>(),
        vec!["h:local", "h:mars", UNSURE]
    );
}

#[tokio::test]
async fn one_candidate_left_asks_nothing() {
    let w = world();
    w.on("assist");
    w.store
        .lock()
        .unwrap()
        .update_host_probe("mars", false, None, None, NOON)
        .unwrap();
    let fake = Fake::answering(vec![says("h:local", 0.9)]);
    assert_eq!(
        propose(&w.ctx(&fake), &w.cache, &w.args()).await.unwrap(),
        None
    );
    assert_eq!(fake.calls(), 0, "the numbers decided");
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn unsure_a_weak_answer_or_a_host_not_offered_proposes_nothing() {
    for answer in [says(UNSURE, 0.9), says("h:mars", 0.3), says("h:venus", 0.9)] {
        let w = world();
        w.on("assist");
        let fake = Fake::answering(vec![answer]);
        assert_eq!(
            propose(&w.ctx(&fake), &w.cache, &w.args()).await.unwrap(),
            None
        );
        assert_eq!(fake.calls(), 1);
    }
}

#[tokio::test]
async fn a_persons_start_confirms_or_corrects_the_proposal_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says("h:mars", 0.8)]);
    propose(&w.ctx(&fake), &w.cache, &w.args())
        .await
        .unwrap()
        .unwrap();
    {
        let s = w.store.lock().unwrap();
        assert!(record_start(&s, w.project, "local", NOON).unwrap());
        assert!(
            !record_start(&s, w.project, "mars", NOON).unwrap(),
            "already decided"
        );
    }
    let r = &w.runs()[0];
    assert_eq!(r.followup.as_deref(), Some("corrected"));
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says("h:mars", 0.9)]);
    let ctx = w.ctx(&fake);
    let input = input_for(&w.store, &w.cache, w.project, NOON)
        .unwrap()
        .unwrap();
    assert_eq!(mode_for(&ctx, &input), Some(Mode::Shadow));
    assert_eq!(ask(&ctx, &input).await, None);
    let runs = w.runs();
    assert_eq!(
        (runs[0].mode.as_str(), runs[0].answer.as_deref()),
        ("shadow", Some("h:mars"))
    );
    let s = w.store.lock().unwrap();
    assert!(
        !record_start(&s, w.project, "mars", NOON).unwrap(),
        "nobody saw it"
    );
}
