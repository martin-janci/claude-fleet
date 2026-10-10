//! `runs { list }` (Orbit Fleet redesign step 8.3, migration 141): one
//! newest-first list of everything that ran on the fleet's behalf, as a
//! `UNION ALL` over four tables that each already record one kind of run.
//! The Automation screen's Runs list reads it (step 8.4).
//!
//! | source          | table                  | what one row is                                   |
//! |-----------------|------------------------|---------------------------------------------------|
//! | `task`          | `tasks`                | one dispatched unit of work (020), in its worker  |
//! | `orchestration` | `orchestration_events` | one action or brake of a mission's loop (115)     |
//! | `jev`           | `decision_runs`        | one decision Jev was asked for (069)              |
//! | `aux`           | `aux_usage`            | one of fleet's own `claude -p` runs (127)         |
//! | `routine`       | `routine_runs`         | one fire of a routine, skipped ones included (131)|
//!
//! How each table's columns map onto a [`RunRow`] is written next to its
//! branch below ([`TASKS_BRANCH`], [`ORCH_BRANCH`], [`JEV_BRANCH`],
//! [`AUX_BRANCH`], [`ROUTINE_BRANCH`]).
//!
//! **Who may see which row is the service's to decide** (`service::runs`),
//! not this file's: `store/` compares columns and has no `ViewScope` (R6-l).
//! The service hands the answer down as a [`RunsReach`] — the session ids,
//! mission ids and whether whole-fleet spend is visible — and every branch
//! applies it in SQL, so `total` and the page agree and no page comes back
//! short.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

/// Every [`RunRow::kind`]: who ran it.
///
/// - `operator`: a task dispatched with no requester session (the
///   operator's own `dispatch_task`, the desktop, a background session);
/// - `task`: a task one session dispatched to another;
/// - `mission`: a mission's run: a task that is an attempt at one of its
///   items, or an action / brake of its loop;
/// - `jev`: a decision Jev was asked for;
/// - `planner`, `summary`, `commit_message`, `release_note`,
///   `morning_brief`, `brief`, `watch_summary`, `triage`, `context_help`: fleet's own
///   `claude -p` runs, by origin ([`super::AUX_ORIGINS`]);
/// - `routine`: a routine's fire (a schedule, an event or Run now).
pub const RUN_KINDS: &[&str] = &[
    "operator",
    "task",
    "mission",
    "jev",
    "planner",
    "summary",
    "commit_message",
    "release_note",
    "morning_brief",
    "brief",
    "watch_summary",
    "triage",
    "context_help",
    "routine",
];

/// Every [`RunRow::outcome`], in plain words.
///
/// - `ok`: it did what it was for;
/// - `failed`: it failed, with [`RunRow::error`] saying how;
/// - `needs_person`: it stopped on a person (a worker that reports itself
///   blocked, a mission's brake, a Jev proposal nobody decided yet);
/// - `nothing_to_do`: it changed nothing (Jev not asked, or unsure; a
///   routine fire that was skipped);
/// - `running`: not over yet.
pub const RUN_OUTCOMES: &[&str] = &["ok", "failed", "needs_person", "nothing_to_do", "running"];

/// Every [`RunRow::source`].
pub const RUN_SOURCES: &[&str] = &["task", "orchestration", "jev", "aux", "routine"];

/// A page's default size.
pub const RUNS_DEFAULT_LIMIT: i64 = 50;
/// A page's largest size.
pub const RUNS_MAX_LIMIT: i64 = 200;

/// The `orchestration_events.kind`s that are runs: what the loop or a person
/// DID to a mission (`step`, `refused`) and the brakes that stopped it on a
/// person (`budget`, `no_progress`). Everything else in that log is audit
/// (`created`, `state`, `item_added`, …), and `planned` is the planner's
/// run, which `aux_usage` books with its cost.
pub const MISSION_RUN_EVENTS: &[&str] = &["step", "refused", "budget", "no_progress"];

/// One run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRow {
    /// `<source>:<rowid>`, stable for the row's life.
    pub id: String,
    /// One of [`RUN_SOURCES`].
    pub source: String,
    /// One of [`RUN_KINDS`].
    pub kind: String,
    /// Who ran it, for a person: a mission's name, a session's name,
    /// `operator`, a Jev use case, `summary`, a routine's name.
    pub owner: String,
    /// Unix seconds.
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// One of [`RUN_OUTCOMES`].
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Micro-USD; absent where the source books no cost (a task's spend is
    /// its worker session's usage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<i64>,
    /// The sessions it ran in or acted on, the worker first. Empty for a
    /// run that ran in no session (a Jev run about a tracker, a planner).
    #[serde(default)]
    pub session_ids: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The routine a fire belongs to (`source = routine` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<i64>,
}

/// What a caller may see, resolved by `service::runs` from its `ViewScope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunsReach {
    /// The hub's own unnarrowed reader (the standalone desktop).
    All,
    /// Everyone else.
    Scoped {
        /// The sessions the caller sees. A task needs every session it
        /// names in here (and is refused once detached); a summary run
        /// needs its session; a Jev run about a session needs that session.
        sessions: Vec<i64>,
        /// The missions the caller may read: their actions, brakes and
        /// planner runs.
        missions: Vec<i64>,
        /// The routines the caller may read: their fires.
        routines: Vec<i64>,
        /// The caller sees every session there is and no org fence: it is
        /// served whole-fleet spend (`org_spend::sees_all_spend`), so the
        /// runs that belong to no session or mission (Jev, a summary whose
        /// conversation no session holds) too.
        spend: bool,
    },
}

/// `runs { list }`'s filters. Every field narrows; `None` does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsFilter {
    /// Started at or after (unix seconds).
    pub since: Option<i64>,
    /// Started before (unix seconds).
    pub until: Option<i64>,
    /// One of [`RUN_KINDS`].
    pub kind: Option<String>,
    /// One of [`RUN_OUTCOMES`].
    pub outcome: Option<String>,
    pub org_id: Option<i64>,
    pub mission_id: Option<i64>,
    /// Runs that ran in or acted on this session.
    pub session_id: Option<i64>,
    /// One routine's fires.
    pub routine_id: Option<i64>,
    /// Clamped into `1..=RUNS_MAX_LIMIT`.
    pub limit: i64,
    pub offset: i64,
    pub reach: RunsReach,
}

impl Default for RunsFilter {
    fn default() -> Self {
        RunsFilter {
            since: None,
            until: None,
            kind: None,
            outcome: None,
            org_id: None,
            mission_id: None,
            session_id: None,
            routine_id: None,
            limit: RUNS_DEFAULT_LIMIT,
            offset: 0,
            reach: RunsReach::All,
        }
    }
}

impl RunsFilter {
    /// `E_INVALID` for a kind or outcome outside the vocabulary.
    pub fn validate(&self) -> Result<(), IpcError> {
        let check = |what: &str, v: &Option<String>, set: &[&str]| match v {
            Some(x) if !set.contains(&x.as_str()) => Err(IpcError::new(
                codes::E_INVALID,
                format!("{what} must be one of {}", set.join(" | ")),
            )),
            _ => Ok(()),
        };
        check("kind", &self.kind, RUN_KINDS)?;
        check("outcome", &self.outcome, RUN_OUTCOMES)
    }
}

/// Every branch selects these columns, in this order.
const COLUMNS: &str = "src, rid, kind, owner, started_at, ended_at, duration_ms, outcome, \
                       error, cost_micros, model, host, org_id, mission_id, s1, s2, summary, \
                       routine_id";

/// `tasks` (alias `t`): one dispatched unit of work.
///
/// - started_at: `created_at`, the dispatch (a queued task has no
///   `started_at` yet, and the index is on `created_at`); ended_at:
///   `finished_at`; duration: `finished_at - COALESCE(started_at,
///   created_at)`.
/// - outcome: `queued` / `running` → running; `done` → ok, or needs_person
///   when the worker's own report (`result_json`, 117) says `blocked`;
///   `failed` → failed; `cancelled` → failed, error `cancelled` unless the
///   row says why.
/// - kind: `mission` when its work item is in a mission
///   (`work_items.orchestration_project_id`), else `operator` when no
///   session requested it, else `task`; owner: the mission's name,
///   `operator`, or the requester session's name.
/// - sessions: the worker, then the requester. host: the worker's host.
///   org: the worker's (else the requester's) session org, filled after
///   the page ([`Store::fill_task_orgs`]): it is an expression per row,
///   too dear to sort a whole table by.
/// - cost / model: none. A task's spend is its worker session's usage, not
///   the task's.
/// - summary: the prompt's first line, at most 120 characters.
pub const TASKS_BRANCH: &str = "SELECT 'task' AS src, t.id AS rid, \
       CASE WHEN wi.orchestration_project_id IS NOT NULL THEN 'mission' \
            WHEN t.requester_session_id IS NULL AND t.detached_at IS NULL THEN 'operator' \
            ELSE 'task' END AS kind, \
       CASE WHEN wi.orchestration_project_id IS NOT NULL \
              THEN COALESCE(op.name, 'mission') \
            WHEN t.requester_session_id IS NULL AND t.detached_at IS NULL THEN 'operator' \
            ELSE COALESCE(rs.friendly_name, rs.tmux_name, 'session') END AS owner, \
       t.created_at AS started_at, t.finished_at AS ended_at, \
       CASE WHEN t.finished_at IS NOT NULL \
            THEN (t.finished_at - COALESCE(t.started_at, t.created_at)) * 1000 END AS duration_ms, \
       CASE t.state WHEN 'done' THEN \
              CASE WHEN json_valid(t.result_json) \
                    AND json_extract(t.result_json, '$.outcome') = 'blocked' \
                   THEN 'needs_person' ELSE 'ok' END \
            WHEN 'failed' THEN 'failed' WHEN 'cancelled' THEN 'failed' \
            ELSE 'running' END AS outcome, \
       CASE t.state WHEN 'failed' THEN COALESCE(t.error, 'failed') \
            WHEN 'cancelled' THEN COALESCE(t.error, 'cancelled') END AS error, \
       NULL AS cost_micros, NULL AS model, ws.host_alias AS host, NULL AS org_id, \
       wi.orchestration_project_id AS mission_id, \
       t.worker_session_id AS s1, t.requester_session_id AS s2, \
       substr(CASE WHEN instr(t.prompt, char(10)) > 0 \
                   THEN substr(t.prompt, 1, instr(t.prompt, char(10)) - 1) \
                   ELSE t.prompt END, 1, 120) AS summary, NULL AS routine_id \
     FROM tasks t \
     LEFT JOIN work_items wi ON wi.id = t.work_item_id \
     LEFT JOIN orchestration_projects op ON op.id = wi.orchestration_project_id \
     LEFT JOIN sessions ws ON ws.id = t.worker_session_id \
     LEFT JOIN sessions rs ON rs.id = t.requester_session_id";

/// The task an orchestration event is about: the column when set, else the
/// `task_id` a `step` event carries in its payload.
const ORCH_TASK: &str = "COALESCE(e.task_id, CASE WHEN json_valid(e.payload) \
     THEN CAST(json_extract(e.payload, '$.task_id') AS INTEGER) END)";

/// `orchestration_events` (alias `e`, its mission `m`), only the
/// [`MISSION_RUN_EVENTS`].
///
/// - started_at: `at`. An action is one moment: no end, no duration.
/// - outcome: `step` → ok; `refused` → failed, error = the payload's
///   `detail` (a step) or `why` (the planner); `budget` / `no_progress` →
///   needs_person (the brake paused the mission and asked a person).
/// - kind `mission`; owner: the mission's name; summary: who acted (`loop`,
///   `planner` or a person) and what (`step: detail`, a brake's `why`).
/// - sessions: the worker of the task a step started. org / mission: the
///   mission's. No cost, model or host: the run it started is its own
///   `task` row.
pub const ORCH_BRANCH: &str = "SELECT 'orchestration' AS src, e.id AS rid, 'mission' AS kind, \
       m.name AS owner, e.at AS started_at, NULL AS ended_at, NULL AS duration_ms, \
       CASE e.kind WHEN 'step' THEN 'ok' WHEN 'refused' THEN 'failed' \
            ELSE 'needs_person' END AS outcome, \
       CASE WHEN e.kind = 'refused' AND json_valid(e.payload) \
            THEN COALESCE(json_extract(e.payload, '$.detail'), json_extract(e.payload, '$.why'), \
                          'refused') \
            WHEN e.kind = 'refused' THEN 'refused' END AS error, \
       NULL AS cost_micros, NULL AS model, NULL AS host, m.org_id AS org_id, \
       e.orchestration_project_id AS mission_id, \
       (SELECT tt.worker_session_id FROM tasks tt WHERE tt.id = {ORCH_TASK}) AS s1, NULL AS s2, \
       e.actor || ': ' || CASE WHEN json_valid(e.payload) THEN \
            CASE WHEN e.kind IN ('step', 'refused') AND json_extract(e.payload, '$.step') IS NOT NULL \
                 THEN json_extract(e.payload, '$.step') || COALESCE(' ' || \
                      json_extract(e.payload, '$.detail'), '') \
                 ELSE COALESCE(json_extract(e.payload, '$.why'), e.kind) END \
            ELSE e.kind END AS summary, NULL AS routine_id \
     FROM orchestration_events e \
     JOIN orchestration_projects m ON m.id = e.orchestration_project_id";

/// `decision_runs` (alias `d`), only the rows that CALLED the provider
/// (`called = 1`): a fallback before any request (the flag, mode or org
/// off, no key, budget, breaker open) ran nothing, and would bury the real
/// runs. Never the offline benchmark's (`subject_kind = 'bench'`): those
/// ran on nobody's behalf.
///
/// - started_at: `at`; duration: `latency_ms` (absent when nothing was
///   sent); ended_at: `at` plus it.
/// - outcome: a call that failed (`timeout`, `http_error`,
///   `rate_limited`, `invalid_answer`) → failed, error = the fallback; any
///   other fallback after the call (low confidence) → nothing_to_do; an
///   `unsure` answer, or one that proposes nothing (`none`, control_route's
///   `control`) → nothing_to_do; an applied answer (turn_outcome,
///   routine_run_outcome) → ok; an `assist` proposal nobody has followed up
///   yet → needs_person; else ok.
/// - kind `jev`; owner: the use case (`feature`); model: `model_version`,
///   else the provider; cost: `cost_microusd`; org: `org_id`.
/// - sessions: the subject, when the run was about a session
///   (`subject_kind = 'session'`); none otherwise (a tracker section, a
///   work start).
pub const JEV_BRANCH: &str = "SELECT 'jev' AS src, d.id AS rid, 'jev' AS kind, \
       d.feature AS owner, d.at AS started_at, \
       CASE WHEN d.latency_ms IS NOT NULL THEN d.at + d.latency_ms / 1000 END AS ended_at, \
       d.latency_ms AS duration_ms, \
       CASE WHEN d.fallback IN ('timeout', 'http_error', 'rate_limited', 'invalid_answer') \
              THEN 'failed' \
            WHEN d.fallback IS NOT NULL THEN 'nothing_to_do' \
            WHEN d.answer IS NULL OR d.answer IN ('unsure', 'none') \
              OR (d.feature = 'control_route' AND d.answer = 'control') \
              THEN 'nothing_to_do' \
            WHEN d.feature IN ('turn_outcome', 'routine_run_outcome') THEN 'ok' \
            WHEN d.mode = 'assist' AND d.followup IS NULL THEN 'needs_person' \
            ELSE 'ok' END AS outcome, \
       CASE WHEN d.fallback IN ('timeout', 'http_error', 'rate_limited', 'invalid_answer') \
            THEN d.fallback END AS error, \
       d.cost_microusd AS cost_micros, COALESCE(d.model_version, d.provider) AS model, \
       NULL AS host, d.org_id AS org_id, NULL AS mission_id, \
       CASE WHEN d.subject_kind = 'session' THEN CAST(d.subject_id AS INTEGER) END AS s1, \
       NULL AS s2, \
       d.feature || ' (' || d.mode || '): ' || COALESCE(d.answer, d.fallback, 'no answer') \
         AS summary, NULL AS routine_id \
     FROM decision_runs d";

/// The session holding a summary run's conversation (the newest, when a
/// resumed conversation has more than one).
const AUX_SESSION: &str = "(SELECT MAX(xs.id) FROM sessions xs \
     WHERE a.claude_session_id IS NOT NULL AND xs.claude_session_id = a.claude_session_id)";

/// `aux_usage` (alias `a`): fleet's own `claude -p` runs (8.2).
///
/// - started_at: `at`, which is when the run was booked: as it finished.
///   No end or duration is recorded.
/// - outcome: ok. Only a run `claude` answered is booked; one that failed
///   is the mission's `refused` row, or the summary's error, elsewhere.
/// - kind / owner: the origin; a planner's owner is its mission's name.
/// - cost, model, host, org, mission: the row's own. sessions: the session
///   that holds a summary's conversation, when one does.
/// - summary: its tokens in and out.
pub const AUX_BRANCH: &str = "SELECT 'aux' AS src, a.id AS rid, a.origin AS kind, \
       CASE WHEN a.origin = 'planner' THEN COALESCE(am.name, 'planner') ELSE a.origin END \
         AS owner, \
       a.at AS started_at, NULL AS ended_at, NULL AS duration_ms, 'ok' AS outcome, \
       NULL AS error, a.cost_micros AS cost_micros, a.model AS model, a.host_alias AS host, \
       a.org_id AS org_id, a.mission_id AS mission_id, {AUX_SESSION} AS s1, NULL AS s2, \
       CASE WHEN a.input_tokens IS NOT NULL OR a.output_tokens IS NOT NULL \
            THEN COALESCE(a.input_tokens, 0) || ' tokens in, ' || \
                 COALESCE(a.output_tokens, 0) || ' out' END AS summary, NULL AS routine_id \
     FROM aux_usage a \
     LEFT JOIN orchestration_projects am ON am.id = a.mission_id";

/// `routine_runs` (alias `rr`, its routine `r`, migration 131): one fire.
///
/// - started_at: `started_at`; ended_at: `finished_at`; duration: their
///   difference.
/// - outcome: `running` → running; `done` → ok; `failed` → failed, error =
///   `reason`; `skipped` → nothing_to_do (the overlap rule, a budget, a
///   person's skip: `reason` says which).
/// - kind `routine`; owner: the routine's name; cost: `cost_micros`, what
///   its session spent; host / org: the routine's. No model: the session's
///   own.
/// - sessions: the session it started (none for a skipped fire).
/// - summary: the trigger (`cron`, `event`, `run_now`) and the reason.
pub const ROUTINE_BRANCH: &str = "SELECT 'routine' AS src, rr.id AS rid, 'routine' AS kind, \
       r.name AS owner, rr.started_at AS started_at, rr.finished_at AS ended_at, \
       CASE WHEN rr.finished_at IS NOT NULL \
            THEN (rr.finished_at - rr.started_at) * 1000 END AS duration_ms, \
       CASE rr.state \
            WHEN 'done' THEN CASE rr.outcome WHEN 'needs_person' THEN 'needs_person' \
                 WHEN 'nothing' THEN 'nothing_to_do' WHEN 'failed' THEN 'failed' ELSE 'ok' END \
            WHEN 'failed' THEN 'failed' \
            WHEN 'skipped' THEN 'nothing_to_do' ELSE 'running' END AS outcome, \
       CASE WHEN rr.state = 'failed' OR rr.outcome = 'failed' \
            THEN COALESCE(rr.reason, 'failed') END AS error, \
       rr.cost_micros AS cost_micros, NULL AS model, r.host_alias AS host, \
       r.org_id AS org_id, NULL AS mission_id, rr.session_id AS s1, NULL AS s2, \
       rr.trigger || COALESCE(': ' || rr.reason, '') AS summary, \
       rr.routine_id AS routine_id \
     FROM routine_runs rr \
     JOIN routines r ON r.id = rr.routine_id";

/// The kinds and outcomes a source can produce, so a filter that no row of
/// a source can match drops the whole branch instead of scanning it.
fn branch_can_match(src: &str, f: &RunsFilter) -> bool {
    let (kinds, outcomes): (&[&str], &[&str]) = match src {
        "task" => (
            &["operator", "task", "mission"],
            &["ok", "failed", "needs_person", "running"],
        ),
        "orchestration" => (&["mission"], &["ok", "failed", "needs_person"]),
        "jev" => (&["jev"], &["ok", "failed", "needs_person", "nothing_to_do"]),
        "aux" => (super::AUX_ORIGINS, &["ok"]),
        _ => (
            &["routine"],
            &["ok", "failed", "needs_person", "nothing_to_do", "running"],
        ),
    };
    f.kind.as_deref().is_none_or(|k| kinds.contains(&k))
        && f.outcome.as_deref().is_none_or(|o| outcomes.contains(&o))
        // Jev runs and routine fires belong to no mission.
        && !((src == "jev" || src == "routine") && f.mission_id.is_some())
        // Only a routine fire belongs to a routine.
        && (src == "routine" || f.routine_id.is_none())
}

/// A JSON array of ids, for `IN (SELECT value FROM json_each(?))`.
fn id_list(ids: &[i64]) -> Value {
    Value::Text(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
}

const IN_IDS: &str = "(SELECT value FROM json_each(?))";

/// One branch's WHERE clauses and their parameters, in order.
struct Where {
    clauses: Vec<String>,
    params: Vec<Value>,
}

impl Where {
    fn new() -> Self {
        Where {
            clauses: Vec::new(),
            params: Vec::new(),
        }
    }
    fn push(&mut self, clause: impl Into<String>, params: impl IntoIterator<Item = Value>) {
        self.clauses.push(clause.into());
        self.params.extend(params);
    }
    fn sql(&self) -> String {
        if self.clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.clauses.join(" AND "))
        }
    }
}

/// The union for `f`, without ORDER BY or LIMIT, and its parameters: each
/// branch carries its own time, filter and reach clauses, on its own
/// indexed columns.
pub(crate) fn union_sql(f: &RunsFilter) -> (String, Vec<Value>) {
    let mut parts: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let int = Value::Integer;
    for src in RUN_SOURCES {
        if !branch_can_match(src, f) {
            continue;
        }
        let mut w = Where::new();
        let (base, time_col) = match *src {
            "task" => (TASKS_BRANCH.to_string(), "t.created_at"),
            "orchestration" => (ORCH_BRANCH.replace("{ORCH_TASK}", ORCH_TASK), "e.at"),
            "jev" => (JEV_BRANCH.to_string(), "d.at"),
            "aux" => (AUX_BRANCH.replace("{AUX_SESSION}", AUX_SESSION), "a.at"),
            _ => (ROUTINE_BRANCH.to_string(), "rr.started_at"),
        };
        match *src {
            "orchestration" => w.push(
                format!(
                    "e.kind IN ({})",
                    MISSION_RUN_EVENTS
                        .iter()
                        .map(|k| format!("'{k}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                [],
            ),
            "jev" => w.push("d.called = 1 AND d.subject_kind <> 'bench'", []),
            _ => {}
        }
        if let Some(since) = f.since {
            w.push(format!("{time_col} >= ?"), [int(since)]);
        }
        if let Some(until) = f.until {
            w.push(format!("{time_col} < ?"), [int(until)]);
        }
        if let Some(org) = f.org_id {
            match *src {
                "task" => w.push(
                    format!(
                        "COALESCE(t.worker_session_id, t.requester_session_id) IN \
                         (SELECT s.id FROM sessions s WHERE {} = ?)",
                        crate::session_org_sql!("s")
                    ),
                    [int(org)],
                ),
                "orchestration" => w.push("m.org_id = ?", [int(org)]),
                "jev" => w.push("d.org_id = ?", [int(org)]),
                "aux" => w.push("a.org_id = ?", [int(org)]),
                _ => w.push("r.org_id = ?", [int(org)]),
            }
        }
        if let Some(mission) = f.mission_id {
            match *src {
                "task" => w.push(
                    "t.work_item_id IN (SELECT id FROM work_items \
                     WHERE orchestration_project_id = ?)",
                    [int(mission)],
                ),
                "orchestration" => w.push("e.orchestration_project_id = ?", [int(mission)]),
                _ => w.push("a.mission_id = ?", [int(mission)]),
            }
        }
        if let Some(routine) = f.routine_id {
            w.push("rr.routine_id = ?", [int(routine)]);
        }
        if let Some(sid) = f.session_id {
            match *src {
                "task" => w.push(
                    "(t.worker_session_id = ? OR t.requester_session_id = ?)",
                    [int(sid), int(sid)],
                ),
                "orchestration" => w.push(
                    format!("{ORCH_TASK} IN (SELECT id FROM tasks WHERE worker_session_id = ?)"),
                    [int(sid)],
                ),
                "jev" => w.push(
                    "d.subject_kind = 'session' AND d.subject_id = ?",
                    [Value::Text(sid.to_string())],
                ),
                "aux" => w.push(
                    "a.claude_session_id IS NOT NULL AND a.claude_session_id = \
                     (SELECT claude_session_id FROM sessions WHERE id = ?)",
                    [int(sid)],
                ),
                _ => w.push("rr.session_id = ?", [int(sid)]),
            }
        }
        if let RunsReach::Scoped {
            sessions,
            missions,
            routines,
            spend,
        } = &f.reach
        {
            let (s, m) = (id_list(sessions), id_list(missions));
            match *src {
                // `service::tasks::task_visible_in_scope_pure`, less its
                // pane-proof clause (an agent's reach to the OTHER end of
                // its own task on its host): narrower, never wider.
                "task" => w.push(
                    format!(
                        "t.detached_at IS NULL \
                         AND (t.requester_session_id IS NOT NULL \
                              OR t.worker_session_id IS NOT NULL) \
                         AND (t.requester_session_id IS NULL \
                              OR t.requester_session_id IN {IN_IDS}) \
                         AND (t.worker_session_id IS NULL OR t.worker_session_id IN {IN_IDS})"
                    ),
                    [s.clone(), s],
                ),
                "orchestration" => w.push(format!("e.orchestration_project_id IN {IN_IDS}"), [m]),
                "jev" if *spend => {}
                "jev" => w.push(
                    format!(
                        "d.subject_kind = 'session' AND CAST(d.subject_id AS INTEGER) IN {IN_IDS}"
                    ),
                    [s],
                ),
                "aux" if *spend => {}
                "aux" => w.push(
                    format!("(a.mission_id IN {IN_IDS} OR {AUX_SESSION} IN {IN_IDS})"),
                    [m, s],
                ),
                // `service::routines::sees_routine`: a fire is its routine's.
                _ => w.push(format!("rr.routine_id IN {IN_IDS}"), [id_list(routines)]),
            }
        }
        parts.push(format!("{base}{}", w.sql()));
        params.extend(w.params);
    }
    if parts.is_empty() {
        // Nothing can match: one empty branch keeps the shape.
        parts.push(format!(
            "SELECT {} WHERE 0",
            COLUMNS
                .split(", ")
                .map(|c| format!("NULL AS {}", c.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    (parts.join(" UNION ALL "), params)
}

/// The outer filters on the computed columns (kind, outcome), over the
/// union aliased `u`.
fn outer_where(f: &RunsFilter, params: &mut Vec<Value>) -> String {
    let mut clauses = Vec::new();
    if let Some(k) = &f.kind {
        clauses.push("u.kind = ?");
        params.push(Value::Text(k.clone()));
    }
    if let Some(o) = &f.outcome {
        clauses.push("u.outcome = ?");
        params.push(Value::Text(o.clone()));
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    }
}

/// The page query and its parameters (also what the query-plan test
/// explains).
pub(crate) fn page_sql(f: &RunsFilter) -> (String, Vec<Value>) {
    let (union, mut params) = union_sql(f);
    let outer = outer_where(f, &mut params);
    params.push(Value::Integer(f.limit.clamp(1, RUNS_MAX_LIMIT)));
    params.push(Value::Integer(f.offset.max(0)));
    (
        format!(
            "SELECT {COLUMNS} FROM ({union}) u{outer} \
             ORDER BY u.started_at DESC, u.src, u.rid DESC LIMIT ? OFFSET ?"
        ),
        params,
    )
}

fn count_sql(f: &RunsFilter) -> (String, Vec<Value>) {
    let (union, mut params) = union_sql(f);
    let outer = outer_where(f, &mut params);
    (format!("SELECT COUNT(*) FROM ({union}) u{outer}"), params)
}

impl Store {
    /// One page of runs, newest first, and how many match in all.
    pub fn runs_list(&self, f: &RunsFilter) -> Result<(Vec<RunRow>, i64), IpcError> {
        f.validate()?;
        let (sql, params) = page_sql(f);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            let src: String = r.get(0)?;
            let rid: i64 = r.get(1)?;
            let s1: Option<i64> = r.get(14)?;
            let s2: Option<i64> = r.get(15)?;
            let routine_id: Option<i64> = r.get(17)?;
            let mut session_ids: Vec<i64> = Vec::with_capacity(2);
            for s in [s1, s2].into_iter().flatten() {
                if !session_ids.contains(&s) {
                    session_ids.push(s);
                }
            }
            Ok(RunRow {
                id: format!("{src}:{rid}"),
                source: src,
                kind: r.get(2)?,
                owner: r.get(3)?,
                started_at: r.get(4)?,
                ended_at: r.get(5)?,
                duration_ms: r.get(6)?,
                outcome: r.get(7)?,
                error: r.get(8)?,
                cost_micros: r.get(9)?,
                model: r.get(10)?,
                host: r.get(11)?,
                org_id: r.get(12)?,
                mission_id: r.get(13)?,
                session_ids,
                summary: r.get(16)?,
                routine_id,
            })
        })?;
        let mut page = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        self.fill_task_orgs(&mut page)?;
        let (sql, params) = count_sql(f);
        let total: i64 = self
            .conn
            .query_row(&sql, rusqlite::params_from_iter(params), |r| r.get(0))?;
        Ok((page, total))
    }

    /// A task row's org: its worker's (else its requester's) session org,
    /// for the page only.
    fn fill_task_orgs(&self, page: &mut [RunRow]) -> Result<(), IpcError> {
        let sql = format!(
            "SELECT {} FROM sessions s WHERE s.id = ?1",
            crate::session_org_sql!("s")
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        for row in page.iter_mut().filter(|r| r.source == "task") {
            let Some(&sid) = row.session_ids.first() else {
                continue;
            };
            let org: Option<Option<i64>> = {
                use rusqlite::OptionalExtension;
                stmt.query_row([sid], |r| r.get(0)).optional()?
            };
            row.org_id = org.flatten();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
