//! Agent runtime: the built-in agents that work on their own under a
//! person's rule, the PR reviewer and the support agent. Design:
//! `docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md`.
//!
//! **Nothing happens without a person's rule.** A rule (`agent_rules`,
//! migration 173) names an agent, what it watches (a repo, a Jira project),
//! where its runs happen and how far it may go:
//!
//! | level     | what the agent does                                        |
//! |-----------|------------------------------------------------------------|
//! | `watch`   | runs and keeps the result in fleet; writes nothing outside |
//! | `propose` | queues each write as an action a person answers            |
//! | `act`     | writes; a kind the agent always asks about is still queued |
//!
//! Only a person writes a rule (`fleet-hub agents grant`, or the `agents`
//! tool on the owner's own device, [`admin`]).
//!
//! **Two loops**, both stopped by `automation.paused`:
//!
//! * `agents_poll` asks each active rule's [`AgentPoller`] what is new, once
//!   per `agents.poll_interval_secs`, and records the jobs it found with the
//!   rule's cursor in one transaction. A job's `(agent, dedup_key)` is
//!   unique, so a poll that finds the same thing again writes nothing, across
//!   restarts too. A poll never runs Claude.
//! * `agents_run` puts jobs whose lease ran out back to pending, claims due
//!   jobs (at most `agents.max_concurrent`, one per subject) and hands each
//!   to its agent's [`AgentRunner`]. How a run ended decides the job's next
//!   state ([`plan_end`]): a transient failure is tried again with backoff,
//!   at most `agents.max_retries` times.
//!
//! The pollers and runners are injected ([`Registry`]), as the shepherd's
//! executor is; production registers the PR reviewer ([`review`]).

pub mod admin;
pub mod debug;
pub mod review;
pub mod support;
#[cfg(test)]
mod tests;
pub mod trace;
pub mod vehicle;
pub mod workspace;

use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{
    AgentActionRow, AgentCursorRow, AgentJobEnd, AgentJobRow, AgentRuleRow, NewAgentJob, Store,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a claimed job is the runner's before another pass may take it
/// again: longer than any run's own time cap.
pub const JOB_LEASE_SECS: i64 = 30 * 60;

/// The first retry's delay; each further one doubles it.
pub const RETRY_BASE_SECS: i64 = 60;

/// How long a job waits after a rate limit, and after a spending limit.
pub const RATE_LIMIT_WAIT_SECS: i64 = 120;
pub const SPENDING_LIMIT_WAIT_SECS: i64 = 3600;

/// What a poll found: new jobs and where the next poll starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Polled {
    pub jobs: Vec<NewAgentJob>,
    /// The poller's own text; `None` clears it.
    pub cursor: Option<String>,
}

/// Why a run failed, which decides whether and when it is tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Worth another try after a backoff.
    Transient,
    /// Will fail the same way again (not found, refused, unparseable).
    Permanent,
    RateLimit,
    SpendingLimit,
    Overloaded,
    /// The run used every turn it had.
    MaxTurns,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Transient => "transient",
            ErrorKind::Permanent => "permanent",
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::SpendingLimit => "spending_limit",
            ErrorKind::Overloaded => "overloaded",
            ErrorKind::MaxTurns => "max_turns",
        }
    }
}

/// How one run of a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Finished; `result` is the run's JSON answer.
    Done {
        result: Option<String>,
        cost_micros: i64,
    },
    /// Nothing to do after all (a draft, a diff too large).
    Skipped { reason: String },
    /// Not yet: the job goes back to pending until `at` (a cooldown), so
    /// the trigger it carries is handled later rather than dropped.
    Deferred { at: i64, reason: String },
    /// Finished, and a person has to look (an approval, a question).
    NeedsPerson {
        result: Option<String>,
        reason: String,
        cost_micros: i64,
    },
    Failed {
        kind: ErrorKind,
        message: String,
        cost_micros: i64,
    },
}

/// One agent's way of finding work.
#[async_trait::async_trait]
pub trait AgentPoller: Send + Sync {
    /// What is new for `rule` since `cursor`. The jobs' `rule_id` and
    /// `agent` are set by the runtime, whatever the poller put there.
    async fn poll(
        &self,
        rule: &AgentRuleRow,
        cursor: Option<&AgentCursorRow>,
        now: i64,
    ) -> Result<Polled, String>;
}

/// One agent's way of doing a job.
#[async_trait::async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run `job` once. `rule` is `None` when the rule was revoked after the
    /// job was found; a runner then normally skips it.
    async fn run(&self, job: &AgentJobRow, rule: Option<&AgentRuleRow>) -> RunOutcome;
}

/// What an executor did with an approved action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionResult {
    Executed,
    /// It ran, and this line says what it made (`created task 42`,
    /// `started fix-x on mercury`); kept on the row so the person sees it.
    Done(String),
    /// It ran and failed; the reason is kept.
    Failed(String),
    /// It no longer applies (the head moved, the ticket closed); not run.
    Expired(String),
}

/// One agent's way of carrying out an approved action (post a review,
/// comment on a ticket).
#[async_trait::async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn execute(&self, action: &AgentActionRow) -> ActionResult;
}

/// One registered agent.
pub struct AgentImpl {
    pub name: &'static str,
    pub poller: Option<Box<dyn AgentPoller>>,
    pub runner: Option<Box<dyn AgentRunner>>,
    pub executor: Option<Box<dyn AgentExecutor>>,
}

/// The agents this process runs.
#[derive(Default)]
pub struct Registry {
    pub agents: Vec<AgentImpl>,
}

impl Registry {
    /// The production agents: the PR reviewer, the support agent and the
    /// generic runner of the catalog's fleet agents.
    pub fn live(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> Self {
        Registry {
            agents: vec![
                review::agent(store, ssh),
                support::agent(store, ssh),
                crate::service::fleet_agents::runner::agent(store, ssh),
            ],
        }
    }

    fn poller(&self, agent: &str) -> Option<&dyn AgentPoller> {
        self.agents
            .iter()
            .find(|a| a.name == agent)
            .and_then(|a| a.poller.as_deref())
    }

    fn runner(&self, agent: &str) -> Option<&dyn AgentRunner> {
        self.agents
            .iter()
            .find(|a| a.name == agent)
            .and_then(|a| a.runner.as_deref())
    }

    fn executor(&self, agent: &str) -> Option<&dyn AgentExecutor> {
        self.agents
            .iter()
            .find(|a| a.name == agent)
            .and_then(|a| a.executor.as_deref())
    }

    fn runnable(&self) -> Vec<&'static str> {
        self.agents
            .iter()
            .filter(|a| a.runner.is_some())
            .map(|a| a.name)
            .collect()
    }
}

/// PURE: the store's record of a run that ended as `outcome`, the job's
/// `attempts`-th run (counting this one). A permanent failure ends the
/// job; `max_turns` gets one more try; a limit waits its fixed time; any
/// other failure waits `RETRY_BASE_SECS · 2^(attempts-1)`. Past
/// `max_retries` attempts a failure is final. A deferral (`error_kind`
/// "deferred") gives its attempt back when the store records it.
pub fn plan_end(outcome: &RunOutcome, attempts: u32, max_retries: u32, now: i64) -> AgentJobEnd {
    match outcome {
        RunOutcome::Done { result, .. } => AgentJobEnd::Final {
            state: "done",
            result: result.clone(),
            error_kind: None,
            error: None,
        },
        RunOutcome::Skipped { reason } => AgentJobEnd::Final {
            state: "skipped",
            result: None,
            error_kind: None,
            error: Some(reason.clone()),
        },
        RunOutcome::Deferred { at, reason } => AgentJobEnd::Retry {
            at: (*at).max(now),
            error_kind: "deferred".into(),
            error: reason.clone(),
        },
        RunOutcome::NeedsPerson { result, reason, .. } => AgentJobEnd::Final {
            state: "needs_person",
            result: result.clone(),
            error_kind: None,
            error: Some(reason.clone()),
        },
        RunOutcome::Failed { kind, message, .. } => {
            let cap = match kind {
                ErrorKind::Permanent => 0,
                ErrorKind::MaxTurns => max_retries.min(2),
                _ => max_retries,
            };
            if attempts >= cap {
                return AgentJobEnd::Final {
                    state: "failed",
                    result: None,
                    error_kind: Some(kind.as_str().to_string()),
                    error: Some(message.clone()),
                };
            }
            let wait = match kind {
                ErrorKind::RateLimit => RATE_LIMIT_WAIT_SECS,
                ErrorKind::SpendingLimit => SPENDING_LIMIT_WAIT_SECS,
                _ => RETRY_BASE_SECS.saturating_mul(1i64 << attempts.saturating_sub(1).min(20)),
            };
            AgentJobEnd::Retry {
                at: now.saturating_add(wait),
                error_kind: kind.as_str().to_string(),
                error: message.clone(),
            }
        }
    }
}

fn cost_of(outcome: &RunOutcome) -> i64 {
    match outcome {
        RunOutcome::Done { cost_micros, .. }
        | RunOutcome::NeedsPerson { cost_micros, .. }
        | RunOutcome::Failed { cost_micros, .. } => *cost_micros,
        RunOutcome::Skipped { .. } | RunOutcome::Deferred { .. } => 0,
    }
}

fn get_u32(s: &Store, key: &str) -> u32 {
    settings::get_string(s, key).parse().unwrap_or(0)
}

/// The rules due a poll at `now`: active, with a poller, and not polled
/// within `interval_secs`.
fn due_rules(
    s: &Store,
    reg: &Registry,
    interval_secs: i64,
    now: i64,
) -> Vec<(AgentRuleRow, Option<AgentCursorRow>)> {
    let Ok(rules) = s.list_agent_rules() else {
        return Vec::new();
    };
    rules
        .into_iter()
        .filter(|r| r.active_at(now) && reg.poller(&r.agent).is_some())
        .filter_map(|r| {
            let cursor = s.agent_cursor(r.id).ok().flatten();
            let due = cursor
                .as_ref()
                .is_none_or(|c| c.polled_at.saturating_add(interval_secs) <= now);
            due.then_some((r, cursor))
        })
        .collect()
}

/// One poll pass. Returns how many new jobs were recorded.
pub async fn poll_with(store: &Mutex<Store>, reg: &Registry, now: i64) -> usize {
    let due = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        if crate::service::loops::paused(&s) {
            return 0;
        }
        let interval = settings::get_secs(&s, settings::AGENTS_POLL_INTERVAL_SECS) as i64;
        if interval == 0 {
            return 0;
        }
        due_rules(&s, reg, interval, now)
    };
    let mut added = 0;
    for (rule, cursor) in due {
        let Some(poller) = reg.poller(&rule.agent) else {
            continue;
        };
        let polled = match poller.poll(&rule, cursor.as_ref(), now).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(rule_id = rule.id, agent = %rule.agent, error = %e, "[agents] poll failed");
                debug::poll_failed(rule.id, now, &e.to_string());
                continue;
            }
        };
        let jobs: Vec<NewAgentJob> = polled
            .jobs
            .into_iter()
            .map(|j| NewAgentJob {
                rule_id: rule.id,
                agent: rule.agent.clone(),
                ..j
            })
            .collect();
        let Ok(s) = store.lock() else {
            return added;
        };
        // Pause all while the poll was out: record nothing it found.
        if crate::service::loops::paused(&s) {
            return added;
        }
        // The rule may have been revoked while the poll was out; its jobs
        // would then belong to nothing.
        if !matches!(s.agent_rule(rule.id), Ok(Some(_))) {
            continue;
        }
        match s.record_agent_poll(rule.id, now, polled.cursor.as_deref(), &jobs) {
            Ok(n) => {
                debug::poll_ok(rule.id);
                added += n
            }
            Err(e) => {
                tracing::warn!(rule_id = rule.id, error = %e, "[agents] recording a poll failed");
                debug::poll_failed(rule.id, now, &format!("recording the poll: {e}"));
            }
        }
    }
    added
}

/// One run pass. Returns how many jobs ran.
pub async fn run_with(store: &Mutex<Store>, reg: &Registry, now: i64) -> usize {
    let runnable = reg.runnable();
    let (jobs, rules, max_retries) = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        if crate::service::loops::paused(&s) {
            return 0;
        }
        if let Err(e) = s.release_stale_agent_jobs(now) {
            tracing::warn!(error = %e, "[agents] releasing stale jobs failed");
        }
        let limit = get_u32(&s, settings::AGENTS_MAX_CONCURRENT) as usize;
        let jobs = match s.claim_agent_jobs(&runnable, now, limit, JOB_LEASE_SECS) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "[agents] claiming jobs failed");
                return 0;
            }
        };
        let rules: Vec<Option<AgentRuleRow>> = jobs
            .iter()
            .map(|j| {
                j.rule_id
                    .and_then(|id| s.agent_rule(id).ok().flatten())
                    .filter(|r| r.active_at(now))
            })
            .collect();
        for job in &jobs {
            trace::started(&s, job, now);
        }
        (jobs, rules, get_u32(&s, settings::AGENTS_MAX_RETRIES))
    };
    let runs = jobs.iter().zip(&rules).map(|(job, rule)| async move {
        let started = std::time::Instant::now();
        let outcome = match reg.runner(&job.agent) {
            Some(r) => r.run(job, rule.as_ref()).await,
            None => RunOutcome::Failed {
                kind: ErrorKind::Permanent,
                message: format!("no runner for agent {}", job.agent),
                cost_micros: 0,
            },
        };
        (outcome, started.elapsed().as_millis() as i64)
    });
    let outcomes = futures_util::future::join_all(runs).await;
    let ended = crate::store::now_unix().max(now);
    {
        let Ok(s) = store.lock() else {
            return 0;
        };
        for (job, (outcome, ms)) in jobs.iter().zip(&outcomes) {
            let end = plan_end(outcome, job.attempts, max_retries, ended);
            if let Err(e) = s.finish_agent_job(job.id, &end, cost_of(outcome), ended) {
                tracing::warn!(job_id = job.id, error = %e, "[agents] recording a run failed");
            }
            trace::ended(&s, job, outcome, &end, *ms, ended);
        }
    }
    jobs.len() + execute_approved(store, reg).await
}

/// Most approved actions one pass carries out.
pub const ACTIONS_PER_PASS: usize = 10;

/// How long a claimed action may go without an answer before it is taken
/// as abandoned (its process stopped mid-way) and failed, never re-run.
pub const ACTION_ABANDONED_SECS: i64 = 3600;

/// Carry out the approved actions (a person's approval, or an `act`
/// rule's own): each by its agent's executor, recorded executed, failed or
/// expired. An action whose agent has no executor waits. Returns how many
/// were carried out.
pub async fn execute_approved(store: &Mutex<Store>, reg: &Registry) -> usize {
    let due = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        if crate::service::loops::paused(&s) {
            return 0;
        }
        let before = crate::store::now_unix() - ACTION_ABANDONED_SECS;
        if let Err(e) = s.fail_abandoned_agent_actions(before) {
            tracing::warn!(error = %e, "[agents] failing abandoned actions failed");
        }
        s.approved_agent_actions(ACTIONS_PER_PASS)
            .unwrap_or_default()
    };
    let mut done = 0;
    for (agent, action) in due {
        let Some(exec) = reg.executor(&agent) else {
            continue;
        };
        // Claimed before the side effect: a second pass, another process on
        // this store, or a restart after a crash never carries it out again.
        let claimed = match store.lock() {
            Ok(s) => s
                .claim_agent_action(action.id, crate::store::now_unix())
                .unwrap_or(false),
            Err(_) => return done,
        };
        if !claimed {
            continue;
        }
        let result = exec.execute(&action).await;
        let (state, error) = match &result {
            ActionResult::Executed => ("executed", None),
            ActionResult::Done(line) => ("executed", Some(line.as_str())),
            ActionResult::Failed(e) => ("failed", Some(e.as_str())),
            ActionResult::Expired(e) => ("expired", Some(e.as_str())),
        };
        let Ok(s) = store.lock() else {
            return done;
        };
        if let Err(e) = s.finish_agent_action(action.id, state, error, crate::store::now_unix()) {
            tracing::warn!(action_id = action.id, error = %e, "[agents] recording an action failed");
        }
        done += 1;
    }
    done
}

/// Holds a loop's single-flight flag until dropped.
struct Flight(&'static AtomicBool);

impl Drop for Flight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn take_flight(flag: &'static AtomicBool) -> Option<Flight> {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| Flight(flag))
}

/// Reconcile-tick hook: [`poll_with`] in its own single-flight task, so a
/// slow `gh` or Jira call never holds the tick. A spawn while one is still
/// running is a no-op.
pub fn spawn_poll(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, period: Duration) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let Some(flight) = take_flight(&RUNNING) else {
        return;
    };
    let (store, reg) = (Arc::clone(store), Registry::live(store, ssh));
    tokio::spawn(async move {
        let _flight = flight;
        let n = poll_with(&store, &reg, crate::store::now_unix()).await;
        if n > 0 {
            tracing::info!("agents: {n} new job(s)");
        }
        crate::service::loops::report("agents_poll", Ok::<_, String>(()), Some(period));
    });
}

/// Reconcile-tick hook: [`run_with`] in its own single-flight task; a run
/// takes minutes.
pub fn spawn_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, period: Duration) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let Some(flight) = take_flight(&RUNNING) else {
        return;
    };
    let (store, reg) = (Arc::clone(store), Registry::live(store, ssh));
    tokio::spawn(async move {
        let _flight = flight;
        let n = run_with(&store, &reg, crate::store::now_unix()).await;
        if n > 0 {
            tracing::info!("agents: ran {n} job(s)");
        }
        crate::service::loops::report("agents_run", Ok::<_, String>(()), Some(period));
    });
}
