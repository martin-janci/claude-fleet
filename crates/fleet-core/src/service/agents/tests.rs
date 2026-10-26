//! The agent runtime's loops against a fake poller and runner.

use super::*;
use crate::store::NewAgentRule;
use std::sync::atomic::AtomicUsize;

const NOW: i64 = 1_000_000;

struct FakePoller {
    polls: AtomicUsize,
    found: Vec<(&'static str, &'static str)>,
}

#[async_trait::async_trait]
impl AgentPoller for FakePoller {
    async fn poll(
        &self,
        _rule: &AgentRuleRow,
        _cursor: Option<&AgentCursorRow>,
        _now: i64,
    ) -> Result<Polled, String> {
        let n = self.polls.fetch_add(1, Ordering::SeqCst);
        Ok(Polled {
            jobs: self
                .found
                .iter()
                .map(|(subject, key)| NewAgentJob {
                    // The runtime sets these, whatever the poller says.
                    rule_id: -1,
                    agent: "forged".into(),
                    subject: (*subject).into(),
                    trigger: "opened".into(),
                    dedup_key: (*key).into(),
                    not_before: 0,
                })
                .collect(),
            cursor: Some(format!("c{n}")),
        })
    }
}

struct FakeRunner {
    runs: Arc<AtomicUsize>,
    outcome: RunOutcome,
}

#[async_trait::async_trait]
impl AgentRunner for FakeRunner {
    async fn run(&self, _job: &AgentJobRow, rule: Option<&AgentRuleRow>) -> RunOutcome {
        assert!(rule.is_some(), "an active rule's job carries its rule");
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.outcome.clone()
    }
}

struct FakeExecutor;

#[async_trait::async_trait]
impl AgentExecutor for FakeExecutor {
    async fn execute(&self, action: &crate::store::AgentActionRow) -> ActionResult {
        match action.kind.as_str() {
            "ok" => ActionResult::Executed,
            "made" => ActionResult::Done("created task 42".into()),
            "late" => ActionResult::Expired("moved".into()),
            _ => ActionResult::Failed("boom".into()),
        }
    }
}

/// A registry with one fake `review` agent, and its runner's run count.
fn registry(
    found: Vec<(&'static str, &'static str)>,
    outcome: RunOutcome,
) -> (Registry, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let reg = Registry {
        agents: vec![AgentImpl {
            name: "review",
            poller: Some(Box::new(FakePoller {
                polls: AtomicUsize::new(0),
                found,
            })),
            runner: Some(Box::new(FakeRunner {
                runs: Arc::clone(&runs),
                outcome,
            })),
            executor: Some(Box::new(FakeExecutor)),
        }],
    };
    (reg, runs)
}

fn store_with_rule(expires_at: Option<i64>) -> (Mutex<Store>, i64) {
    let s = Store::open_in_memory().unwrap();
    let id = s
        .grant_agent_rule(&NewAgentRule {
            agent: "review".into(),
            scope: "gh:o/r".into(),
            level: "watch".into(),
            host_alias: "local".into(),
            project_id: None,
            config: "{}".into(),
            granted_by: "console".into(),
            granted_at: 0,
            expires_at,
            definition: None,
            bindings: "[]".into(),
        })
        .unwrap();
    (Mutex::new(s), id)
}

fn done() -> RunOutcome {
    RunOutcome::Done {
        result: Some(r#"{"ok":true}"#.into()),
        cost_micros: 40,
    }
}

#[tokio::test]
async fn a_poll_records_its_jobs_once_per_interval() {
    let (st, rid) = store_with_rule(None);
    let (reg, _) = registry(vec![("gh:o/r#1", "a"), ("gh:o/r#2", "b")], done());
    assert_eq!(poll_with(&st, &reg, NOW).await, 2);
    assert_eq!(poll_with(&st, &reg, NOW + 10).await, 0, "not due yet");
    assert_eq!(poll_with(&st, &reg, NOW + 300).await, 0, "due, nothing new");
    let s = st.lock().unwrap();
    let jobs = s.list_agent_jobs(None, 10).unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs
        .iter()
        .all(|j| j.rule_id == Some(rid) && j.agent == "review"));
    assert_eq!(
        s.agent_cursor(rid).unwrap().unwrap().cursor.as_deref(),
        Some("c1")
    );
}

#[tokio::test]
async fn an_expired_rule_and_an_interval_of_zero_poll_nothing() {
    let (st, _) = store_with_rule(Some(NOW));
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    assert_eq!(poll_with(&st, &reg, NOW).await, 0);
    let (st, _) = store_with_rule(None);
    settings::set(
        &st.lock().unwrap(),
        settings::AGENTS_POLL_INTERVAL_SECS,
        "0",
    )
    .unwrap();
    assert_eq!(poll_with(&st, &reg, NOW).await, 0);
}

#[tokio::test]
async fn a_run_finishes_its_jobs_and_keeps_their_answer() {
    let (st, _) = store_with_rule(None);
    let (reg, runs) = registry(vec![("gh:o/r#1", "a"), ("gh:o/r#2", "b")], done());
    poll_with(&st, &reg, NOW).await;
    assert_eq!(run_with(&st, &reg, NOW).await, 2);
    assert_eq!(run_with(&st, &reg, NOW).await, 0, "nothing left");
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    let s = st.lock().unwrap();
    for j in s.list_agent_jobs(None, 10).unwrap() {
        assert_eq!(j.state, "done");
        assert_eq!(j.result.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(j.cost_micros, 40);
    }
}

#[tokio::test]
async fn max_concurrent_caps_a_pass() {
    let (st, _) = store_with_rule(None);
    settings::set(&st.lock().unwrap(), settings::AGENTS_MAX_CONCURRENT, "1").unwrap();
    let (reg, _) = registry(vec![("gh:o/r#1", "a"), ("gh:o/r#2", "b")], done());
    poll_with(&st, &reg, NOW).await;
    assert_eq!(run_with(&st, &reg, NOW).await, 1);
    assert_eq!(run_with(&st, &reg, NOW).await, 1);
}

#[tokio::test]
async fn a_transient_failure_is_retried_until_the_cap() {
    let (st, _) = store_with_rule(None);
    let failed = RunOutcome::Failed {
        kind: ErrorKind::Transient,
        message: "timeout".into(),
        cost_micros: 1,
    };
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], failed);
    poll_with(&st, &reg, NOW).await;
    // The pass records the end at the wall clock or later; jump far ahead
    // each time so the backoff has passed.
    let mut t = crate::store::now_unix().max(NOW);
    for _ in 0..3 {
        assert_eq!(run_with(&st, &reg, t).await, 1);
        t += 3600;
    }
    assert_eq!(
        run_with(&st, &reg, t).await,
        0,
        "three attempts, then final"
    );
    let j = &st.lock().unwrap().list_agent_jobs(None, 1).unwrap()[0];
    assert_eq!((j.state.as_str(), j.attempts), ("failed", 3));
    assert_eq!(j.error_kind.as_deref(), Some("transient"));
    assert_eq!(j.cost_micros, 3);
}

/// Redesign 8.1: Pause all stops both loops before they poll or run.
#[tokio::test]
async fn pause_all_stops_the_agents() {
    let (st, _) = store_with_rule(None);
    let (reg, runs) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    settings::set(&st.lock().unwrap(), settings::AUTOMATION_PAUSED, "true").unwrap();
    assert_eq!(poll_with(&st, &reg, NOW + 600).await, 0);
    assert_eq!(run_with(&st, &reg, NOW + 600).await, 0);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    let s = st.lock().unwrap();
    assert_eq!(s.list_agent_jobs(None, 10).unwrap()[0].state, "pending");
}

#[tokio::test]
async fn a_job_without_a_runner_is_never_claimed() {
    let (st, _) = store_with_rule(None);
    let (mut reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    reg.agents[0].runner = None;
    assert_eq!(run_with(&st, &reg, NOW).await, 0);
    assert_eq!(
        st.lock().unwrap().list_agent_jobs(None, 1).unwrap()[0].state,
        "pending"
    );
}

#[test]
fn plan_end_follows_the_error_kind() {
    let fail = |kind| RunOutcome::Failed {
        kind,
        message: "m".into(),
        cost_micros: 0,
    };
    let retry_at = |end: AgentJobEnd| match end {
        AgentJobEnd::Retry { at, .. } => Some(at),
        AgentJobEnd::Final { .. } => None,
    };
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::Transient), 1, 3, 0)),
        Some(60)
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::Overloaded), 2, 3, 0)),
        Some(120)
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::Transient), 3, 3, 0)),
        None
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::Permanent), 1, 3, 0)),
        None
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::MaxTurns), 1, 3, 0)),
        Some(60)
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::MaxTurns), 2, 3, 0)),
        None
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::RateLimit), 1, 3, 0)),
        Some(RATE_LIMIT_WAIT_SECS)
    );
    assert_eq!(
        retry_at(plan_end(&fail(ErrorKind::SpendingLimit), 1, 3, 0)),
        Some(SPENDING_LIMIT_WAIT_SECS)
    );
    match plan_end(
        &RunOutcome::Skipped {
            reason: "draft".into(),
        },
        1,
        3,
        0,
    ) {
        AgentJobEnd::Final { state, error, .. } => {
            assert_eq!((state, error.as_deref()), ("skipped", Some("draft")))
        }
        other => panic!("{other:?}"),
    }
}

/// A deferred run (a cooldown) goes back to pending at its time, however
/// many attempts it has used: the trigger it carries is not dropped.
#[test]
fn a_deferred_run_waits_until_its_time_rather_than_ending() {
    let deferred = |at| RunOutcome::Deferred {
        at,
        reason: "cooldown".into(),
    };
    match plan_end(&deferred(500), 9, 3, 100) {
        AgentJobEnd::Retry { at, error, .. } => assert_eq!((at, error.as_str()), (500, "cooldown")),
        other => panic!("{other:?}"),
    }
    // Never in the past.
    assert!(matches!(
        plan_end(&deferred(50), 1, 3, 100),
        AgentJobEnd::Retry { at: 100, .. }
    ));
}

#[tokio::test]
async fn approved_actions_are_carried_out_and_pending_ones_wait() {
    let (st, _) = store_with_rule(None);
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    let ids = {
        let s = st.lock().unwrap();
        let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
        let ok = s
            .insert_approved_agent_action(jid, "ok", "{}", "rule:act", NOW)
            .unwrap();
        let late = s
            .insert_approved_agent_action(jid, "late", "{}", "rule:act", NOW)
            .unwrap();
        let bad = s
            .insert_approved_agent_action(jid, "bad", "{}", "rule:act", NOW)
            .unwrap();
        let waiting = s.insert_agent_action(jid, "ok", "{}", NOW).unwrap();
        [ok, late, bad, waiting]
    };
    settings::set(&st.lock().unwrap(), settings::AUTOMATION_PAUSED, "true").unwrap();
    assert_eq!(execute_approved(&st, &reg).await, 0, "Pause all holds them");
    settings::set(&st.lock().unwrap(), settings::AUTOMATION_PAUSED, "false").unwrap();
    assert_eq!(execute_approved(&st, &reg).await, 3);
    let s = st.lock().unwrap();
    let state = |id: i64| {
        s.list_agent_actions(None, 10)
            .unwrap()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .state
    };
    assert_eq!(
        ids.map(state),
        ["executed", "expired", "failed", "pending"].map(String::from)
    );
}

/// Review w08-3: what an approved action made reaches the person: the line
/// is kept on the row and `status.recent` carries it (a person's approvals
/// only, not a rule's own).
#[tokio::test]
async fn a_done_line_is_kept_and_status_shows_recent_outcomes() {
    let (st, _) = store_with_rule(None);
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    let (made, bad, by_rule) = {
        let s = st.lock().unwrap();
        let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
        let made = s.insert_agent_action(jid, "made", "{}", NOW).unwrap();
        let bad = s.insert_agent_action(jid, "bad", "{}", NOW).unwrap();
        s.decide_agent_action(made, true, "laptop", NOW).unwrap();
        s.decide_agent_action(bad, true, "phone", NOW).unwrap();
        let by_rule = s
            .insert_approved_agent_action(jid, "made", "{}", "rule:act", NOW)
            .unwrap();
        (made, bad, by_rule)
    };
    assert_eq!(execute_approved(&st, &reg).await, 3);
    let now = crate::store::now_unix();
    let status = admin::status(&st, &admin::AgentsArgs::default(), now).unwrap();
    let recent: Vec<(i64, &str, Option<&str>)> = status
        .recent
        .iter()
        .map(|a| (a.id, a.state.as_str(), a.error.as_deref()))
        .collect();
    assert!(
        recent.contains(&(made, "executed", Some("created task 42"))),
        "{recent:?}"
    );
    assert!(
        recent.contains(&(bad, "failed", Some("boom"))),
        "{recent:?}"
    );
    assert!(!recent.iter().any(|r| r.0 == by_rule), "{recent:?}");
    // A day later they are gone.
    let later = admin::status(
        &st,
        &admin::AgentsArgs::default(),
        now + admin::RECENT_WINDOW_SECS + 1,
    )
    .unwrap();
    assert!(later.recent.is_empty());
}

/// Review w11-5: the pending actions are not cut to the jobs' `limit`, so
/// the Inbox lists (and the badge counts) all of them, the oldest too.
#[tokio::test]
async fn status_lists_every_pending_action_whatever_the_limit() {
    let (st, _) = store_with_rule(None);
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    let first = {
        let s = st.lock().unwrap();
        let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
        let first = s.insert_agent_action(jid, "kb_learn", "{}", NOW).unwrap();
        for i in 1..26 {
            s.insert_agent_action(jid, "kb_learn", "{}", NOW + i)
                .unwrap();
        }
        first
    };
    let args = admin::AgentsArgs {
        limit: Some(20),
        ..Default::default()
    };
    let status = admin::status(&st, &args, NOW + 30).unwrap();
    assert_eq!(status.pending.len(), 26);
    assert_eq!(status.pending_total, 26);
    assert!(status.pending.iter().any(|a| a.id == first));
}

/// Past [`admin::STATUS_PENDING_MAX`] the list is cut, but `pending_total`
/// still counts every waiting action, so the badge does not stop at the cap.
#[tokio::test]
async fn status_counts_pending_actions_past_the_list_cap() {
    let (st, _) = store_with_rule(None);
    let (reg, _) = registry(vec![("gh:o/r#1", "a")], done());
    poll_with(&st, &reg, NOW).await;
    let over = admin::STATUS_PENDING_MAX as i64 + 7;
    {
        let s = st.lock().unwrap();
        let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
        for i in 0..over {
            s.insert_agent_action(jid, "kb_learn", "{}", NOW + i)
                .unwrap();
        }
    }
    let status = admin::status(&st, &admin::AgentsArgs::default(), NOW + over).unwrap();
    assert_eq!(status.pending.len(), admin::STATUS_PENDING_MAX);
    assert_eq!(status.pending_total, over as u64);
}
