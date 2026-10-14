//! Debugging the agents: `agents { action: explain }` says, per rule, whether
//! it polls, when next, what its last poll failed with, and where its jobs
//! stand; `agents { action: job, id }` is one job whole, its proposed
//! actions included. Both only read.
//!
//! A poll that fails was only logged; its rule then looked merely quiet. The
//! process keeps each rule's last poll failure ([`poll_failed`], cleared by
//! a poll that succeeds), as `service::loops` keeps a loop's last error: per
//! process, so a restart forgets it and the next poll says again.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::loops::{self, LoopHealth};
use crate::service::settings;
use crate::store::{AgentActionRow, AgentJobRow, AgentRuleRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// A rule's last poll failure: when, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollError {
    pub at: i64,
    pub message: String,
}

static POLL_ERRORS: Mutex<Option<HashMap<i64, PollError>>> = Mutex::new(None);

/// Record that rule `rule_id`'s poll failed at `at`.
pub fn poll_failed(rule_id: i64, at: i64, message: &str) {
    if let Ok(mut m) = POLL_ERRORS.lock() {
        m.get_or_insert_with(HashMap::new).insert(
            rule_id,
            PollError {
                at,
                message: message.to_string(),
            },
        );
    }
}

/// Rule `rule_id` polled: its last failure is history.
pub fn poll_ok(rule_id: i64) {
    if let Ok(mut m) = POLL_ERRORS.lock() {
        if let Some(m) = m.as_mut() {
            m.remove(&rule_id);
        }
    }
}

fn poll_error(rule_id: i64) -> Option<PollError> {
    POLL_ERRORS
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.get(&rule_id).cloned()))
}

/// One rule, as `explain` reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuleExplain {
    pub rule: AgentRuleRow,
    /// Not expired.
    pub active: bool,
    /// `ok` | `warn` | `block`.
    pub state: String,
    /// One line: what it does next, or why it does nothing.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polled_at: Option<i64>,
    /// When it is due its next poll; `None` while polling is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<i64>,
    /// Where its poller stopped (the agent's own cursor text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_error: Option<PollError>,
    /// Its jobs, counted by state.
    #[serde(default)]
    pub jobs: BTreeMap<String, i64>,
    /// The pending job that runs first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_job: Option<AgentJobRow>,
    /// Its newest failed job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failed: Option<AgentJobRow>,
}

/// `agents { action: explain }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsExplain {
    /// `automation.paused`: neither loop polls nor runs.
    pub automation_paused: bool,
    /// `agents.poll_interval_secs`; 0 turns polling off.
    pub poll_interval_secs: i64,
    pub max_concurrent: i64,
    pub max_retries: i64,
    /// `agents_poll` and `agents_run` in the process that answered.
    #[serde(default)]
    pub loops: Vec<LoopHealth>,
    #[serde(default)]
    pub rules: Vec<AgentRuleExplain>,
}

/// `agents { action: job, id }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentJobDetail {
    pub job: AgentJobRow,
    /// The rule it ran under, while it is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<AgentRuleRow>,
    /// The writes it proposed, oldest first.
    #[serde(default)]
    pub actions: Vec<AgentActionRow>,
    /// Where it stands, in words.
    pub summary: String,
}

fn db(e: rusqlite::Error) -> IpcError {
    IpcError::new(codes::E_INTERNAL, e.to_string())
}

fn int_setting(s: &Store, key: &str) -> i64 {
    settings::get_string(s, key).parse().unwrap_or(0)
}

fn rule_explain(
    s: &Store,
    rule: AgentRuleRow,
    paused: bool,
    interval: i64,
    now: i64,
) -> Result<AgentRuleExplain, IpcError> {
    let active = rule.active_at(now);
    let cursor = s.agent_cursor(rule.id).map_err(db)?;
    let polled_at = cursor.as_ref().map(|c| c.polled_at);
    let next_poll_at =
        (interval > 0).then(|| polled_at.map_or(now, |p| p.saturating_add(interval)));
    let poll_error = poll_error(rule.id);
    let jobs: BTreeMap<String, i64> = s
        .agent_job_counts(rule.id)
        .map_err(db)?
        .into_iter()
        .collect();
    let next_job = s.next_pending_agent_job(rule.id).map_err(db)?;
    let last_failed = s.newest_agent_job_in(rule.id, "failed").map_err(db)?;
    let (state, summary) = if !active {
        (
            "block",
            "Its grant expired: it polls and runs nothing.".to_string(),
        )
    } else if paused {
        (
            "block",
            "Pause all is on (automation.paused): it polls and runs nothing.".to_string(),
        )
    } else if interval == 0 {
        (
            "block",
            "agents.poll_interval_secs is 0: polling is off, so it finds no new work.".to_string(),
        )
    } else if let Some(e) = &poll_error {
        ("warn", format!("Its last poll failed: {}", e.message))
    } else if let Some(j) = &next_job {
        let when = if j.next_attempt_at <= now {
            "the next run pass".to_string()
        } else {
            format!("its retry time ({})", j.next_attempt_at)
        };
        (
            "ok",
            format!("Job {} ({}) waits for {when}.", j.id, j.subject),
        )
    } else if polled_at.is_none() {
        (
            "ok",
            "It has not polled yet: the next poll pass does.".to_string(),
        )
    } else {
        (
            "ok",
            format!("Polls every {interval} s; nothing is waiting."),
        )
    };
    Ok(AgentRuleExplain {
        rule,
        active,
        state: state.into(),
        summary,
        polled_at,
        next_poll_at,
        cursor: cursor.and_then(|c| c.cursor),
        poll_error,
        jobs,
        next_job,
        last_failed,
    })
}

/// `agents { action: explain }`: every rule's poll and queue.
pub fn explain(store: &Mutex<Store>, now: i64) -> Result<AgentsExplain, IpcError> {
    let loop_rows = loops::registry()
        .snapshot()
        .into_iter()
        .filter(|l| l.name == "agents_poll" || l.name == "agents_run")
        .collect();
    let s = lock(store)?;
    let paused = loops::paused(&s);
    let interval = int_setting(&s, settings::AGENTS_POLL_INTERVAL_SECS);
    let mut rules = Vec::new();
    for rule in s.list_agent_rules().map_err(db)? {
        rules.push(rule_explain(&s, rule, paused, interval, now)?);
    }
    Ok(AgentsExplain {
        automation_paused: paused,
        poll_interval_secs: interval,
        max_concurrent: int_setting(&s, settings::AGENTS_MAX_CONCURRENT),
        max_retries: int_setting(&s, settings::AGENTS_MAX_RETRIES),
        loops: loop_rows,
        rules,
    })
}

/// What job `j` is waiting for or came to, in words.
fn job_words(j: &AgentJobRow, now: i64) -> String {
    let err = || {
        let kind = j.error_kind.as_deref().map(|k| format!(" ({k})"));
        format!(
            "{}{}",
            j.error.as_deref().unwrap_or("no message"),
            kind.unwrap_or_default()
        )
    };
    match j.state.as_str() {
        "pending" if j.attempts == 0 && j.next_attempt_at <= now => {
            "Waiting for the next run pass.".to_string()
        }
        "pending" if j.attempts == 0 => format!("Waiting until {}.", j.next_attempt_at),
        "pending" => format!(
            "Attempt {} failed: {}; retries at {}.",
            j.attempts,
            err(),
            j.next_attempt_at
        ),
        "running" => match j.lease_until {
            Some(l) if l < now => "Its run's lease ran out: the next pass releases it.".into(),
            _ => "Running now.".into(),
        },
        "failed" => format!("Failed after {} attempt(s): {}.", j.attempts, err()),
        "skipped" => format!("Skipped: {}.", j.error.as_deref().unwrap_or("no reason")),
        "needs_person" => format!(
            "Needs a person: {}.",
            j.error.as_deref().unwrap_or("no reason")
        ),
        _ => "Done.".into(),
    }
}

/// `agents { action: job, id }`.
pub fn job(store: &Mutex<Store>, id: i64, now: i64) -> Result<AgentJobDetail, IpcError> {
    let s = lock(store)?;
    let job = s
        .agent_job(id)
        .map_err(db)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no job {id}")))?;
    let rule = match job.rule_id {
        Some(r) => s.agent_rule(r).map_err(db)?,
        None => None,
    };
    Ok(AgentJobDetail {
        actions: s.agent_actions_of_job(id).map_err(db)?,
        summary: job_words(&job, now),
        rule,
        job,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewAgentJob, NewAgentRule};

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("box").unwrap();
        let id = s
            .grant_agent_rule(&NewAgentRule {
                agent: "review".into(),
                scope: "gh:o/r".into(),
                level: "watch".into(),
                host_alias: "box".into(),
                project_id: None,
                config: "{}".into(),
                granted_by: "test".into(),
                granted_at: 1,
                expires_at: Some(1_000),
                definition: None,
                bindings: "[]".into(),
            })
            .unwrap();
        (Mutex::new(s), id)
    }

    fn rule(e: &AgentsExplain, id: i64) -> &AgentRuleExplain {
        e.rules.iter().find(|r| r.rule.id == id).unwrap()
    }

    #[test]
    fn explain_says_why_a_rule_finds_no_work() {
        let (st, id) = store();
        let e = explain(&st, 10).unwrap();
        let r = rule(&e, id);
        assert_eq!(r.state, "ok");
        assert!(r.summary.contains("not polled yet"), "{}", r.summary);
        assert_eq!(r.next_poll_at, Some(10), "a rule never polled is due now");
        // A failed poll is kept until one succeeds.
        poll_failed(id, 11, "gh: HTTP 401");
        let e = explain(&st, 12).unwrap();
        assert_eq!(rule(&e, id).state, "warn");
        assert!(rule(&e, id).summary.contains("HTTP 401"));
        poll_ok(id);
        assert!(rule(&explain(&st, 12).unwrap(), id).poll_error.is_none());
        // Polling off, Pause all and an expired grant each block.
        lock(&st)
            .unwrap()
            .set_setting(settings::AGENTS_POLL_INTERVAL_SECS, "0")
            .unwrap();
        let r = explain(&st, 12).unwrap();
        assert!(rule(&r, id).summary.contains("polling is off"));
        assert_eq!(rule(&r, id).next_poll_at, None);
        lock(&st)
            .unwrap()
            .set_setting(settings::AUTOMATION_PAUSED, "true")
            .unwrap();
        let r = explain(&st, 12).unwrap();
        assert!(r.automation_paused);
        assert!(rule(&r, id).summary.contains("Pause all"));
        let r = explain(&st, 2_000).unwrap();
        assert!(rule(&r, id).summary.contains("expired"));
    }

    #[test]
    fn explain_counts_a_rules_jobs_and_names_the_next() {
        let (st, id) = store();
        {
            let s = lock(&st).unwrap();
            let job = |key: &str, at: i64| NewAgentJob {
                rule_id: id,
                agent: "review".into(),
                subject: format!("gh:o/r#{key}"),
                trigger: "poll".into(),
                dedup_key: key.into(),
                not_before: at,
            };
            s.record_agent_poll(id, 5, Some("c1"), &[job("1", 50), job("2", 7)])
                .unwrap();
        }
        let e = explain(&st, 10).unwrap();
        let r = rule(&e, id);
        assert_eq!(r.jobs.get("pending"), Some(&2));
        assert_eq!(r.cursor.as_deref(), Some("c1"));
        assert_eq!(r.polled_at, Some(5));
        let next = r.next_job.as_ref().unwrap();
        assert_eq!(next.subject, "gh:o/r#2", "the earliest first");
        assert!(r.summary.contains("next run pass"), "{}", r.summary);
        // One job, whole.
        let d = job(&st, next.id, 10).unwrap();
        assert_eq!(d.rule.as_ref().map(|r| r.id), Some(id));
        assert!(d.actions.is_empty());
        assert_eq!(d.summary, "Waiting for the next run pass.");
        assert_eq!(job(&st, 9_999, 10).unwrap_err().code, codes::E_NOTFOUND);
    }
}
