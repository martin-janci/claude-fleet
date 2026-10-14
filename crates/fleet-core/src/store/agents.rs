//! Agent runtime rows (migration 173): the rules a person grants the
//! built-in agents, each rule's poll cursor, the jobs the pollers found and
//! the writes a job proposes. The runtime is `service::agents`; the design
//! is `docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md`.

use super::Store;
use rusqlite::{params, OptionalExtension, Row};

/// The agents a rule may name: the built-in ones, and `fleet`, a catalog
/// `fleet_agent` definition (the rule's `definition`).
pub const AGENT_KINDS: [&str; 3] = ["review", "support", "fleet"];

/// What a rule lets its agent do, least first.
pub const AGENT_LEVELS: [&str; 3] = ["watch", "propose", "act"];

/// `agent_jobs.state` values.
pub const AGENT_JOB_STATES: [&str; 6] = [
    "pending",
    "running",
    "done",
    "failed",
    "skipped",
    "needs_person",
];

/// `agent_actions.state` values.
pub const AGENT_ACTION_STATES: [&str; 6] = [
    "pending", "approved", "rejected", "executed", "failed", "expired",
];

/// Longest `config` JSON a rule carries (the migration's CHECK).
pub const AGENT_CONFIG_MAX_CHARS: usize = 8000;

/// Longest `bindings` JSON a rule carries (migration 185's CHECK).
pub const AGENT_BINDINGS_MAX_CHARS: usize = 8000;

/// Longest `definition` name (migration 185's CHECK).
pub const AGENT_DEFINITION_MAX_CHARS: usize = 100;

/// Longest `scope` text (the migration's CHECK).
pub const AGENT_SCOPE_MAX_CHARS: usize = 300;

/// A person's standing rule for one agent on one scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentRuleRow {
    pub id: i64,
    pub agent: String,
    pub scope: String,
    pub level: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    /// The agent's options, a JSON object.
    pub config: String,
    pub granted_by: String,
    pub granted_at: i64,
    pub expires_at: Option<i64>,
    /// `fleet`: the catalog definition the rule runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// `fleet`: the store bindings, a JSON array (`[]` otherwise).
    #[serde(default = "no_bindings")]
    pub bindings: String,
}

fn no_bindings() -> String {
    "[]".into()
}

impl AgentRuleRow {
    /// In force at `now`: not expired.
    pub fn active_at(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|e| e > now)
    }
}

/// A rule to write; `(agent, scope)` replaces an existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgentRule {
    pub agent: String,
    pub scope: String,
    pub level: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    pub config: String,
    pub granted_by: String,
    pub granted_at: i64,
    pub expires_at: Option<i64>,
    /// `fleet` only (and then required).
    pub definition: Option<String>,
    /// A JSON array; `[]` for a built-in agent.
    pub bindings: String,
}

/// Where a rule's poller stopped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentCursorRow {
    pub rule_id: i64,
    pub polled_at: i64,
    pub cursor: Option<String>,
}

/// One job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentJobRow {
    pub id: i64,
    pub rule_id: Option<i64>,
    pub agent: String,
    pub subject: String,
    pub trigger: String,
    pub dedup_key: String,
    pub state: String,
    pub attempts: u32,
    pub next_attempt_at: i64,
    pub lease_until: Option<i64>,
    pub error_kind: Option<String>,
    pub error: Option<String>,
    /// The run's JSON answer.
    pub result: Option<String>,
    pub cost_micros: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A job a poller found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgentJob {
    pub rule_id: i64,
    pub agent: String,
    pub subject: String,
    pub trigger: String,
    pub dedup_key: String,
    /// Not before this time (a debounce); usually now.
    pub not_before: i64,
}

/// How a run of a job ended, as the store records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentJobEnd {
    /// Finished in a final state (`done`, `failed`, `skipped`,
    /// `needs_person`), with the run's answer and error, if any.
    Final {
        state: &'static str,
        result: Option<String>,
        error_kind: Option<String>,
        error: Option<String>,
    },
    /// Failed, to be tried again at `at`.
    Retry {
        at: i64,
        error_kind: String,
        error: String,
    },
}

/// One write a job proposes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentActionRow {
    pub id: i64,
    pub job_id: i64,
    pub kind: String,
    /// Exactly what will be sent, a JSON value.
    pub payload: String,
    pub state: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub executed_at: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
}

const RULE_COLS: &str = "id, agent, scope, level, host_alias, project_id, config, \
                         granted_by, granted_at, expires_at, definition, bindings";
const JOB_COLS: &str = "id, rule_id, agent, subject, trigger, dedup_key, state, attempts, \
                        next_attempt_at, lease_until, error_kind, error, result, cost_micros, \
                        created_at, updated_at";
const ACTION_COLS: &str = "id, job_id, kind, payload, state, decided_by, decided_at, \
                           executed_at, error, created_at";

fn rule_row(r: &Row<'_>) -> rusqlite::Result<AgentRuleRow> {
    Ok(AgentRuleRow {
        id: r.get(0)?,
        agent: r.get(1)?,
        scope: r.get(2)?,
        level: r.get(3)?,
        host_alias: r.get(4)?,
        project_id: r.get(5)?,
        config: r.get(6)?,
        granted_by: r.get(7)?,
        granted_at: r.get(8)?,
        expires_at: r.get(9)?,
        definition: r.get(10)?,
        bindings: r.get(11)?,
    })
}

fn job_row(r: &Row<'_>) -> rusqlite::Result<AgentJobRow> {
    Ok(AgentJobRow {
        id: r.get(0)?,
        rule_id: r.get(1)?,
        agent: r.get(2)?,
        subject: r.get(3)?,
        trigger: r.get(4)?,
        dedup_key: r.get(5)?,
        state: r.get(6)?,
        attempts: r.get(7)?,
        next_attempt_at: r.get(8)?,
        lease_until: r.get(9)?,
        error_kind: r.get(10)?,
        error: r.get(11)?,
        result: r.get(12)?,
        cost_micros: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

fn action_row(r: &Row<'_>) -> rusqlite::Result<AgentActionRow> {
    Ok(AgentActionRow {
        id: r.get(0)?,
        job_id: r.get(1)?,
        kind: r.get(2)?,
        payload: r.get(3)?,
        state: r.get(4)?,
        decided_by: r.get(5)?,
        decided_at: r.get(6)?,
        executed_at: r.get(7)?,
        error: r.get(8)?,
        created_at: r.get(9)?,
    })
}

fn invalid(msg: String) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(msg)
}

impl Store {
    /// Write a rule, replacing the one with the same `(agent, scope)`.
    /// Returns its id. Refuses an unknown agent or level and over-long or
    /// malformed fields before SQLite's CHECK would.
    pub fn grant_agent_rule(&self, rule: &NewAgentRule) -> Result<i64, rusqlite::Error> {
        if !AGENT_KINDS.contains(&rule.agent.as_str()) {
            return Err(invalid(format!(
                "agent must be one of {}",
                AGENT_KINDS.join(", ")
            )));
        }
        if !AGENT_LEVELS.contains(&rule.level.as_str()) {
            return Err(invalid(format!(
                "level must be one of {}",
                AGENT_LEVELS.join(", ")
            )));
        }
        if rule.scope.is_empty() || rule.scope.chars().count() > AGENT_SCOPE_MAX_CHARS {
            return Err(invalid(format!(
                "scope must be 1-{AGENT_SCOPE_MAX_CHARS} characters"
            )));
        }
        if rule.config.chars().count() > AGENT_CONFIG_MAX_CHARS {
            return Err(invalid(format!(
                "config must be at most {AGENT_CONFIG_MAX_CHARS} characters"
            )));
        }
        if !serde_json::from_str::<serde_json::Value>(&rule.config).is_ok_and(|v| v.is_object()) {
            return Err(invalid("config must be a JSON object".into()));
        }
        match (&rule.definition, rule.agent == "fleet") {
            (Some(d), true) if !d.is_empty() && d.chars().count() <= AGENT_DEFINITION_MAX_CHARS => {
            }
            (None, false) => {}
            _ => {
                return Err(invalid(
                    "a fleet rule names its definition, and only a fleet rule does".into(),
                ))
            }
        }
        if rule.bindings.chars().count() > AGENT_BINDINGS_MAX_CHARS
            || !serde_json::from_str::<serde_json::Value>(&rule.bindings)
                .is_ok_and(|v| v.is_array())
        {
            return Err(invalid(format!(
                "bindings must be a JSON array of at most {AGENT_BINDINGS_MAX_CHARS} characters"
            )));
        }
        self.conn.query_row(
            "INSERT INTO agent_rules \
               (agent, scope, level, host_alias, project_id, config, granted_by, granted_at, \
                expires_at, definition, bindings) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(agent, scope) DO UPDATE SET level = excluded.level, \
               host_alias = excluded.host_alias, project_id = excluded.project_id, \
               config = excluded.config, granted_by = excluded.granted_by, \
               granted_at = excluded.granted_at, expires_at = excluded.expires_at, \
               definition = excluded.definition, bindings = excluded.bindings \
             RETURNING id",
            params![
                rule.agent,
                rule.scope,
                rule.level,
                rule.host_alias,
                rule.project_id,
                rule.config,
                rule.granted_by,
                rule.granted_at,
                rule.expires_at,
                rule.definition,
                rule.bindings
            ],
            |r| r.get(0),
        )
    }

    /// Remove one rule. `true` when there was one. Its cursor goes with it;
    /// its jobs stay, without the rule.
    pub fn revoke_agent_rule(&self, id: i64) -> Result<bool, rusqlite::Error> {
        let n = self
            .conn
            .execute("DELETE FROM agent_rules WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Remove every rule (the agents' Pause all). Returns how many went.
    pub fn revoke_all_agent_rules(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM agent_rules", [])
    }

    /// Every rule, expired ones included, by id.
    pub fn list_agent_rules(&self) -> Result<Vec<AgentRuleRow>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {RULE_COLS} FROM agent_rules ORDER BY id"))?;
        let rows = stmt.query_map([], rule_row)?;
        rows.collect()
    }

    /// One rule by id.
    pub fn agent_rule(&self, id: i64) -> Result<Option<AgentRuleRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {RULE_COLS} FROM agent_rules WHERE id = ?1"),
                params![id],
                rule_row,
            )
            .optional()
    }

    /// Where rule `rule_id`'s poller stopped, if it ever ran.
    pub fn agent_cursor(&self, rule_id: i64) -> Result<Option<AgentCursorRow>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT rule_id, polled_at, cursor FROM agent_cursors WHERE rule_id = ?1",
                params![rule_id],
                |r| {
                    Ok(AgentCursorRow {
                        rule_id: r.get(0)?,
                        polled_at: r.get(1)?,
                        cursor: r.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// Record a finished poll of rule `rule_id` and every job it found, in
    /// one transaction, so a crash between the two neither loses the jobs
    /// nor polls past them. A job whose `(agent, dedup_key)` exists already
    /// is not written again. Returns how many jobs are new.
    pub fn record_agent_poll(
        &self,
        rule_id: i64,
        polled_at: i64,
        cursor: Option<&str>,
        jobs: &[NewAgentJob],
    ) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let mut added = 0;
        for j in jobs {
            added += tx.execute(
                "INSERT INTO agent_jobs \
                   (rule_id, agent, subject, trigger, dedup_key, next_attempt_at, created_at, \
                    updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) \
                 ON CONFLICT(agent, dedup_key) DO NOTHING",
                params![
                    j.rule_id,
                    j.agent,
                    j.subject,
                    j.trigger,
                    j.dedup_key,
                    j.not_before.max(polled_at),
                    polled_at
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO agent_cursors (rule_id, polled_at, cursor) VALUES (?1, ?2, ?3) \
             ON CONFLICT(rule_id) DO UPDATE SET polled_at = excluded.polled_at, \
               cursor = excluded.cursor",
            params![rule_id, polled_at, cursor],
        )?;
        tx.commit()?;
        Ok(added)
    }

    /// Put every running job whose lease ran out back to `pending` (its
    /// runner died with the process, or never answered). Returns how many.
    pub fn release_stale_agent_jobs(&self, now: i64) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            "UPDATE agent_jobs SET state = 'pending', lease_until = NULL, updated_at = ?1 \
             WHERE state = 'running' AND (lease_until IS NULL OR lease_until <= ?1)",
            params![now],
        )
    }

    /// Claim up to `limit` due jobs of the given agents: `pending`, due by
    /// `now`, oldest first, at most one per subject, and none for a subject
    /// that already has a running job. Each comes back `running` with a
    /// lease to `now + lease_secs`.
    pub fn claim_agent_jobs(
        &self,
        agents: &[&str],
        now: i64,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<AgentJobRow>, rusqlite::Error> {
        if agents.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let tx = self.conn.unchecked_transaction()?;
        let picked: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT j.id, j.agent, j.subject FROM agent_jobs j \
                 WHERE j.state = 'pending' AND j.next_attempt_at <= ?1 \
                   AND NOT EXISTS (SELECT 1 FROM agent_jobs r WHERE r.state = 'running' \
                                   AND r.agent = j.agent AND r.subject = j.subject) \
                 ORDER BY j.next_attempt_at, j.id",
            )?;
            let mut rows = stmt.query(params![now])?;
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let (id, agent, subject): (i64, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
                if !agents.contains(&agent.as_str()) || !seen.insert((agent, subject)) {
                    continue;
                }
                out.push(id);
                if out.len() == limit {
                    break;
                }
            }
            out
        };
        let mut claimed = Vec::with_capacity(picked.len());
        for id in picked {
            tx.execute(
                "UPDATE agent_jobs SET state = 'running', lease_until = ?2, \
                   attempts = attempts + 1, updated_at = ?3 WHERE id = ?1",
                params![id, now + lease_secs, now],
            )?;
            claimed.push(tx.query_row(
                &format!("SELECT {JOB_COLS} FROM agent_jobs WHERE id = ?1"),
                params![id],
                job_row,
            )?);
        }
        tx.commit()?;
        Ok(claimed)
    }

    /// Record how a run of job `id` ended, with what it cost.
    pub fn finish_agent_job(
        &self,
        id: i64,
        end: &AgentJobEnd,
        cost_micros: i64,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        match end {
            AgentJobEnd::Final {
                state,
                result,
                error_kind,
                error,
            } => {
                if *state == "pending" || *state == "running" || !AGENT_JOB_STATES.contains(state) {
                    return Err(invalid(format!("{state:?} is not a final job state")));
                }
                self.conn.execute(
                    "UPDATE agent_jobs SET state = ?2, result = ?3, error_kind = ?4, \
                       error = ?5, lease_until = NULL, cost_micros = cost_micros + ?6, \
                       updated_at = ?7 WHERE id = ?1",
                    params![id, state, result, error_kind, error, cost_micros, now],
                )?;
            }
            AgentJobEnd::Retry {
                at,
                error_kind,
                error,
            } => {
                self.conn.execute(
                    "UPDATE agent_jobs SET state = 'pending', next_attempt_at = ?2, \
                       error_kind = ?3, error = ?4, lease_until = NULL, \
                       cost_micros = cost_micros + ?5, updated_at = ?6 WHERE id = ?1",
                    params![id, at, error_kind, error, cost_micros, now],
                )?;
            }
        }
        Ok(())
    }

    /// The newest jobs, of one agent or all.
    pub fn list_agent_jobs(
        &self,
        agent: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentJobRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM agent_jobs WHERE ?1 IS NULL OR agent = ?1 \
             ORDER BY created_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![agent, limit as i64], job_row)?;
        rows.collect()
    }

    /// When the newest finished run (`done` or `needs_person`) of another
    /// job on `subject` ended: the loop guard's per-subject cooldown.
    pub fn last_agent_subject_run(
        &self,
        agent: &str,
        subject: &str,
        except_job: i64,
    ) -> Result<Option<i64>, rusqlite::Error> {
        self.conn.query_row(
            "SELECT MAX(updated_at) FROM agent_jobs \
             WHERE agent = ?1 AND subject = ?2 AND id <> ?3 AND state IN ('done', 'needs_person')",
            params![agent, subject, except_job],
            |r| r.get(0),
        )
    }

    /// One job by id.
    pub fn agent_job(&self, id: i64) -> Result<Option<AgentJobRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {JOB_COLS} FROM agent_jobs WHERE id = ?1"),
                params![id],
                job_row,
            )
            .optional()
    }

    /// Rule `rule_id`'s jobs counted by state: `(state, count)`, by state.
    pub fn agent_job_counts(&self, rule_id: i64) -> Result<Vec<(String, i64)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT state, COUNT(*) FROM agent_jobs WHERE rule_id = ?1 \
             GROUP BY state ORDER BY state",
        )?;
        let rows = stmt.query_map(params![rule_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Rule `rule_id`'s newest job in `state`.
    pub fn newest_agent_job_in(
        &self,
        rule_id: i64,
        state: &str,
    ) -> Result<Option<AgentJobRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {JOB_COLS} FROM agent_jobs WHERE rule_id = ?1 AND state = ?2 \
                     ORDER BY updated_at DESC, id DESC LIMIT 1"
                ),
                params![rule_id, state],
                job_row,
            )
            .optional()
    }

    /// Rule `rule_id`'s pending job that runs first: the earliest
    /// `next_attempt_at`.
    pub fn next_pending_agent_job(
        &self,
        rule_id: i64,
    ) -> Result<Option<AgentJobRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {JOB_COLS} FROM agent_jobs WHERE rule_id = ?1 AND state = 'pending' \
                     ORDER BY next_attempt_at, id LIMIT 1"
                ),
                params![rule_id],
                job_row,
            )
            .optional()
    }

    /// The actions job `job_id` proposed, oldest first.
    pub fn agent_actions_of_job(
        &self,
        job_id: i64,
    ) -> Result<Vec<AgentActionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ACTION_COLS} FROM agent_actions WHERE job_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![job_id], action_row)?;
        rows.collect()
    }

    /// Queue a write job `job_id` proposes. `payload` must be JSON.
    pub fn insert_agent_action(
        &self,
        job_id: i64,
        kind: &str,
        payload: &str,
        now: i64,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "INSERT INTO agent_actions (job_id, kind, payload, created_at) \
             VALUES (?1, ?2, ?3, ?4) RETURNING id",
            params![job_id, kind, payload, now],
            |r| r.get(0),
        )
    }

    /// Queue a write already answered: `approved` by the rule itself (an
    /// `act` rule), with `decided_by` saying so. `payload` must be JSON.
    pub fn insert_approved_agent_action(
        &self,
        job_id: i64,
        kind: &str,
        payload: &str,
        decided_by: &str,
        now: i64,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "INSERT INTO agent_actions (job_id, kind, payload, state, decided_by, decided_at, \
               created_at) \
             VALUES (?1, ?2, ?3, 'approved', ?4, ?5, ?5) RETURNING id",
            params![job_id, kind, payload, decided_by, now],
            |r| r.get(0),
        )
    }

    /// Approved actions waiting for their executor, oldest first, each with
    /// the agent whose job proposed it.
    pub fn approved_agent_actions(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, AgentActionRow)>, rusqlite::Error> {
        let cols: String = ACTION_COLS
            .split(", ")
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT j.agent, {cols} FROM agent_actions a JOIN agent_jobs j ON j.id = a.job_id \
             WHERE a.state = 'approved' ORDER BY a.decided_at, a.id LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let agent: String = r.get(0)?;
            Ok((
                agent,
                AgentActionRow {
                    id: r.get(1)?,
                    job_id: r.get(2)?,
                    kind: r.get(3)?,
                    payload: r.get(4)?,
                    state: r.get(5)?,
                    decided_by: r.get(6)?,
                    decided_at: r.get(7)?,
                    executed_at: r.get(8)?,
                    error: r.get(9)?,
                    created_at: r.get(10)?,
                },
            ))
        })?;
        rows.collect()
    }

    /// Record what the executor did with an approved action: `executed`,
    /// `failed` or `expired`, with why when it did not run.
    pub fn finish_agent_action(
        &self,
        id: i64,
        state: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        if !["executed", "failed", "expired"].contains(&state) {
            return Err(invalid(format!("{state:?} is not an executor's answer")));
        }
        self.conn.execute(
            "UPDATE agent_actions SET state = ?2, error = ?3, executed_at = ?4 \
             WHERE id = ?1 AND state = 'approved'",
            params![id, state, error, now],
        )?;
        Ok(())
    }

    /// The newest actions, in one state or all.
    pub fn list_agent_actions(
        &self,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentActionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ACTION_COLS} FROM agent_actions WHERE ?1 IS NULL OR state = ?1 \
             ORDER BY created_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![state, limit as i64], action_row)?;
        rows.collect()
    }

    /// A person's answer to a pending action: `approved` or `rejected`.
    /// Only a `pending` action changes; the answer is the row as it is
    /// afterwards, or `None` when no action has that id.
    pub fn decide_agent_action(
        &self,
        id: i64,
        approve: bool,
        decided_by: &str,
        now: i64,
    ) -> Result<Option<AgentActionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE agent_actions SET state = ?2, decided_by = ?3, decided_at = ?4 \
             WHERE id = ?1 AND state = 'pending'",
            params![
                id,
                if approve { "approved" } else { "rejected" },
                decided_by,
                now
            ],
        )?;
        self.conn
            .query_row(
                &format!("SELECT {ACTION_COLS} FROM agent_actions WHERE id = ?1"),
                params![id],
                action_row,
            )
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(agent: &str, scope: &str) -> NewAgentRule {
        NewAgentRule {
            agent: agent.into(),
            scope: scope.into(),
            level: "watch".into(),
            host_alias: "local".into(),
            project_id: None,
            config: "{}".into(),
            granted_by: "console".into(),
            granted_at: 100,
            expires_at: None,
            definition: None,
            bindings: "[]".into(),
        }
    }

    fn job(rule_id: i64, subject: &str, key: &str) -> NewAgentJob {
        NewAgentJob {
            rule_id,
            agent: "review".into(),
            subject: subject.into(),
            trigger: "opened".into(),
            dedup_key: key.into(),
            not_before: 0,
        }
    }

    #[test]
    fn a_grant_on_the_same_scope_replaces_the_rule() {
        let s = Store::open_in_memory().unwrap();
        let id = s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap();
        let mut again = rule("review", "gh:o/r");
        again.level = "act".into();
        assert_eq!(s.grant_agent_rule(&again).unwrap(), id);
        s.grant_agent_rule(&rule("support", "jira:1:L2S")).unwrap();
        let rules = s.list_agent_rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].level, "act");
        assert!(s.revoke_agent_rule(id).unwrap());
        assert!(!s.revoke_agent_rule(id).unwrap());
        assert_eq!(s.revoke_all_agent_rules().unwrap(), 1);
    }

    #[test]
    fn a_grant_refuses_bad_fields() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.grant_agent_rule(&rule("deploy", "x")).is_err());
        let mut r = rule("review", "gh:o/r");
        r.level = "auto".into();
        assert!(s.grant_agent_rule(&r).is_err());
        let mut r = rule("review", "");
        assert!(s.grant_agent_rule(&r).is_err());
        r.scope = "gh:o/r".into();
        r.config = "[1]".into();
        assert!(s.grant_agent_rule(&r).is_err());
        r.config = "{".into();
        assert!(s.grant_agent_rule(&r).is_err());
        assert!(s.list_agent_rules().unwrap().is_empty());
    }

    #[test]
    fn a_fleet_rule_names_its_definition_and_keeps_its_bindings() {
        let s = Store::open_in_memory().unwrap();
        let mut r = rule("fleet", "l2-prod");
        assert!(s.grant_agent_rule(&r).is_err(), "no definition");
        r.definition = Some("l2-support".into());
        r.bindings = r#"[{"role":"memory","namespace":"rule"}]"#.into();
        let id = s.grant_agent_rule(&r).unwrap();
        let mut staging = r.clone();
        staging.scope = "l2-staging".into();
        assert_ne!(s.grant_agent_rule(&staging).unwrap(), id, "two instances");
        let row = s.agent_rule(id).unwrap().unwrap();
        assert_eq!(row.definition.as_deref(), Some("l2-support"));
        assert_eq!(row.bindings, r.bindings);
        let mut bad = rule("review", "gh:o/r");
        bad.definition = Some("l2-support".into());
        assert!(s.grant_agent_rule(&bad).is_err(), "a built-in names none");
        let mut bad = r.clone();
        bad.bindings = "{}".into();
        assert!(s.grant_agent_rule(&bad).is_err(), "bindings is an array");
        assert_eq!(
            s.agent_rule(s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap())
                .unwrap()
                .unwrap()
                .bindings,
            "[]"
        );
    }

    #[test]
    fn a_poll_writes_each_job_once_and_moves_the_cursor() {
        let s = Store::open_in_memory().unwrap();
        let rid = s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap();
        let jobs = [
            job(rid, "gh:o/r#1", "gh:o/r#1@a"),
            job(rid, "gh:o/r#2", "gh:o/r#2@b"),
        ];
        assert_eq!(
            s.record_agent_poll(rid, 1000, Some("c1"), &jobs).unwrap(),
            2
        );
        assert_eq!(
            s.record_agent_poll(rid, 1300, Some("c2"), &jobs).unwrap(),
            0
        );
        let c = s.agent_cursor(rid).unwrap().unwrap();
        assert_eq!((c.polled_at, c.cursor.as_deref()), (1300, Some("c2")));
        assert_eq!(s.list_agent_jobs(Some("review"), 10).unwrap().len(), 2);
        assert!(s.list_agent_jobs(Some("support"), 10).unwrap().is_empty());
    }

    #[test]
    fn a_claim_takes_one_job_per_subject_and_a_lease_runs_out() {
        let s = Store::open_in_memory().unwrap();
        let rid = s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap();
        let mut later = job(rid, "gh:o/r#3", "gh:o/r#3@c");
        later.not_before = 5000;
        s.record_agent_poll(
            rid,
            1000,
            None,
            &[
                job(rid, "gh:o/r#1", "gh:o/r#1@a"),
                job(rid, "gh:o/r#1", "gh:o/r#1@b"),
                job(rid, "gh:o/r#2", "gh:o/r#2@a"),
                later,
            ],
        )
        .unwrap();
        assert!(s
            .claim_agent_jobs(&["support"], 2000, 10, 60)
            .unwrap()
            .is_empty());
        let got = s.claim_agent_jobs(&["review"], 2000, 10, 60).unwrap();
        let keys: Vec<_> = got.iter().map(|j| j.dedup_key.as_str()).collect();
        assert_eq!(
            keys,
            ["gh:o/r#1@a", "gh:o/r#2@a"],
            "one per subject, not yet due skipped"
        );
        assert!(got.iter().all(|j| j.state == "running" && j.attempts == 1));
        assert!(
            s.claim_agent_jobs(&["review"], 2000, 10, 60)
                .unwrap()
                .is_empty(),
            "a subject with a running job waits"
        );
        assert_eq!(s.release_stale_agent_jobs(2059).unwrap(), 0);
        assert_eq!(s.release_stale_agent_jobs(2060).unwrap(), 2);
        assert_eq!(
            s.claim_agent_jobs(&["review"], 2060, 1, 60).unwrap().len(),
            1
        );
    }

    #[test]
    fn a_finished_job_keeps_its_answer_and_a_retry_waits() {
        let s = Store::open_in_memory().unwrap();
        let rid = s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap();
        s.record_agent_poll(rid, 1000, None, &[job(rid, "gh:o/r#1", "k1")])
            .unwrap();
        let j = &s.claim_agent_jobs(&["review"], 1000, 1, 60).unwrap()[0];
        s.finish_agent_job(
            j.id,
            &AgentJobEnd::Retry {
                at: 1120,
                error_kind: "transient".into(),
                error: "timeout".into(),
            },
            5,
            1010,
        )
        .unwrap();
        assert!(s
            .claim_agent_jobs(&["review"], 1100, 1, 60)
            .unwrap()
            .is_empty());
        let j = &s.claim_agent_jobs(&["review"], 1120, 1, 60).unwrap()[0];
        assert_eq!(j.attempts, 2);
        s.finish_agent_job(
            j.id,
            &AgentJobEnd::Final {
                state: "done",
                result: Some(r#"{"verdict":"APPROVE"}"#.into()),
                error_kind: None,
                error: None,
            },
            7,
            1130,
        )
        .unwrap();
        let done = s.agent_job(j.id).unwrap().unwrap();
        assert_eq!(
            s.last_agent_subject_run("review", "gh:o/r#1", 0).unwrap(),
            Some(1130)
        );
        assert_eq!(
            s.last_agent_subject_run("review", "gh:o/r#1", j.id)
                .unwrap(),
            None
        );
        assert_eq!(done.state, "done");
        assert_eq!(done.cost_micros, 12);
        assert_eq!(done.error, None);
        let bad = AgentJobEnd::Final {
            state: "running",
            result: None,
            error_kind: None,
            error: None,
        };
        assert!(s.finish_agent_job(j.id, &bad, 0, 0).is_err());
    }

    #[test]
    fn an_action_is_decided_once() {
        let s = Store::open_in_memory().unwrap();
        let rid = s.grant_agent_rule(&rule("review", "gh:o/r")).unwrap();
        s.record_agent_poll(rid, 1000, None, &[job(rid, "gh:o/r#1", "k1")])
            .unwrap();
        let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
        let aid = s
            .insert_agent_action(jid, "post_review", r#"{"event":"COMMENT"}"#, 1000)
            .unwrap();
        assert_eq!(s.list_agent_actions(Some("pending"), 10).unwrap().len(), 1);
        let a = s
            .decide_agent_action(aid, true, "phone", 1100)
            .unwrap()
            .unwrap();
        assert_eq!(
            (a.state.as_str(), a.decided_by.as_deref()),
            ("approved", Some("phone"))
        );
        let a = s
            .decide_agent_action(aid, false, "laptop", 1200)
            .unwrap()
            .unwrap();
        assert_eq!(a.state, "approved", "a second answer changes nothing");
        assert_eq!(s.decide_agent_action(aid + 9, true, "x", 0).unwrap(), None);
        assert!(s.insert_agent_action(jid, "x", "not json", 0).is_err());
        let auto = s
            .insert_approved_agent_action(jid, "post_review", "{}", "rule:act", 1300)
            .unwrap();
        let due = s.approved_agent_actions(10).unwrap();
        assert_eq!(
            due.iter()
                .map(|(a, r)| (a.as_str(), r.id))
                .collect::<Vec<_>>(),
            [("review", aid), ("review", auto)]
        );
        s.finish_agent_action(aid, "executed", None, 1400).unwrap();
        s.finish_agent_action(auto, "failed", Some("422"), 1400)
            .unwrap();
        assert!(s.approved_agent_actions(10).unwrap().is_empty());
        assert!(s.finish_agent_action(aid, "approved", None, 0).is_err());
        let failed = &s.list_agent_actions(Some("failed"), 1).unwrap()[0];
        assert_eq!(
            (failed.error.as_deref(), failed.executed_at),
            (Some("422"), Some(1400))
        );
    }
}
