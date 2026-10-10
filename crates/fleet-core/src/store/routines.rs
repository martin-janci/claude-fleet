//! Routines and their runs (migration 131, redesign step 8.5). Who may see
//! and change a routine, the cron grammar, the scheduler tick and the
//! budget and overlap rules are in `service::routines`; this is the rows.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// How long one tick may hold a routine.
pub const ROUTINE_LEASE_SECS: i64 = 120;

/// `routines.trigger` values.
pub const ROUTINE_TRIGGERS: [&str; 3] = ["cron", "event", "manual"];
/// `routines.overlap` values.
pub const ROUTINE_OVERLAPS: [&str; 2] = ["skip", "parallel"];
/// `routines.paused_reason` when the routine's project was removed: it can
/// no longer start a session there.
pub const PAUSED_PROJECT_REMOVED: &str = "paused: its project was removed";
/// `routines.paused_reason` when the routine's host was removed.
pub const PAUSED_HOST_REMOVED: &str = "paused: its host was removed";
/// `routine_runs.state` values; `running` is the only open one.
pub const ROUTINE_RUN_STATES: [&str; 4] = ["running", "done", "failed", "skipped"];

/// One routine, as a reader receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRow {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_person_id: Option<i64>,
    pub name: String,
    pub enabled: bool,
    /// cron | event | manual
    pub trigger: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Minutes east of UTC the cron line is read at.
    #[serde(default)]
    pub utc_offset_min: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// The newest session event already looked at. The scheduler's own.
    #[serde(skip)]
    pub event_cursor: i64,
    pub host_alias: String,
    pub project_id: i64,
    /// The credential profile its sessions bill; `None` = the host's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_run_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_day_micros: Option<i64>,
    /// skip | parallel
    pub overlap: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub skip_next: bool,
    /// Why the scheduler turned it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// A pull request event fires only for this repo (migration 156):
    /// `owner/name`, or `name` of any owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_repo: Option<String>,
    /// `anyone`: a pull request opened by any session of its org; absent
    /// (or `me`) = its owner's sessions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_author: Option<String>,
    /// At most one run per PR (or session) in this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_rate_secs: Option<i64>,
    /// The IANA zone it was saved in (migration 159, M15 G3.8), shown
    /// beside the schedule; `utc_offset_min` still drives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// A run still going after this many seconds is stopped and failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_max_secs: Option<i64>,
    /// Where a run starts when `host_alias` cannot take it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_host: Option<String>,
    /// A failed run is started again once.
    #[serde(default)]
    pub retry_once: bool,
    /// 0 report only | 1 ask before push | 2 push and open PRs; absent = 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<i64>,
}

/// What a person writes: every field of a routine but its identity and the
/// scheduler's own state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutineFields {
    pub name: String,
    pub enabled: bool,
    pub trigger: String,
    pub cron: Option<String>,
    pub utc_offset_min: i64,
    pub event: Option<String>,
    pub host_alias: String,
    pub project_id: i64,
    pub profile: Option<String>,
    pub prompt: String,
    pub budget_run_micros: Option<i64>,
    pub budget_day_micros: Option<i64>,
    pub overlap: String,
    pub event_repo: Option<String>,
    pub event_author: Option<String>,
    pub event_rate_secs: Option<i64>,
    pub time_zone: Option<String>,
    pub run_max_secs: Option<i64>,
    pub fallback_host: Option<String>,
    pub retry_once: bool,
    pub autonomy: Option<i64>,
}

/// One run of a routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRunRow {
    pub id: i64,
    pub routine_id: i64,
    /// cron | event | run_now
    pub trigger: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_ref: Option<String>,
    /// running | done | failed | skipped
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default)]
    pub cost_micros: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<i64>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// What the run came to (migration 133, step 8.10): one of
    /// [`ROUTINE_RUN_OUTCOMES`], `None` until something answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Who answered it: `exit` | `rule` | `jev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_source: Option<String>,
    /// Why it failed, as a code (migration 159): the start's `E_*`, or the
    /// scheduler's own (`service::routines::fix`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// The host its session started on: the routine's, or its fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
}

/// `routine_runs.outcome` (migration 133): every value its `CHECK` admits.
pub const ROUTINE_RUN_OUTCOMES: [&str; 4] = ["did_work", "nothing", "failed", "needs_person"];

/// `routine_runs.outcome_source`, weakest first: an answer never replaces
/// a stronger one, so a failed exit wins over a rule and both over Jev.
pub const ROUTINE_RUN_OUTCOME_SOURCES: [&str; 3] = ["jev", "rule", "exit"];

/// A run to record.
#[derive(Debug, Clone, Default)]
pub struct NewRoutineRun<'a> {
    pub routine_id: i64,
    pub trigger: &'a str,
    pub trigger_ref: Option<&'a str>,
    pub state: &'a str,
    pub reason: Option<&'a str>,
    pub session_id: Option<i64>,
    pub scheduled_for: Option<i64>,
    /// When it fired (the scheduler's clock).
    pub at: i64,
    /// Why it failed, as a code; `None` for a run that did not.
    pub error_code: Option<&'a str>,
    /// The host its session started on.
    pub host_alias: Option<&'a str>,
}

const COLS: &str = "id, org_id, owner_person_id, name, enabled, trigger, cron, utc_offset_min, \
                    event, event_cursor, host_alias, project_id, profile, prompt, \
                    budget_run_micros, budget_day_micros, overlap, next_run_at, skip_next, \
                    paused_reason, created_at, updated_at, event_repo, event_author, \
                    event_rate_secs, time_zone, run_max_secs, fallback_host, retry_once, \
                    autonomy";

fn routine(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRow> {
    Ok(RoutineRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        owner_person_id: r.get(2)?,
        name: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        trigger: r.get(5)?,
        cron: r.get(6)?,
        utc_offset_min: r.get(7)?,
        event: r.get(8)?,
        event_cursor: r.get(9)?,
        host_alias: r.get(10)?,
        project_id: r.get(11)?,
        profile: r.get(12)?,
        prompt: r.get(13)?,
        budget_run_micros: r.get(14)?,
        budget_day_micros: r.get(15)?,
        overlap: r.get(16)?,
        next_run_at: r.get(17)?,
        skip_next: r.get::<_, i64>(18)? != 0,
        paused_reason: r.get(19)?,
        created_at: r.get(20)?,
        updated_at: r.get(21)?,
        event_repo: r.get(22)?,
        event_author: r.get(23)?,
        event_rate_secs: r.get(24)?,
        time_zone: r.get(25)?,
        run_max_secs: r.get(26)?,
        fallback_host: r.get(27)?,
        retry_once: r.get::<_, i64>(28)? != 0,
        autonomy: r.get(29)?,
    })
}

const RUN_COLS: &str = "id, routine_id, trigger, trigger_ref, state, reason, session_id, \
                        cost_micros, scheduled_for, started_at, finished_at, outcome, \
                        outcome_source, error_code, host_alias";

fn run(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRunRow> {
    Ok(RoutineRunRow {
        id: r.get(0)?,
        routine_id: r.get(1)?,
        trigger: r.get(2)?,
        trigger_ref: r.get(3)?,
        state: r.get(4)?,
        reason: r.get(5)?,
        session_id: r.get(6)?,
        cost_micros: r.get(7)?,
        scheduled_for: r.get(8)?,
        started_at: r.get(9)?,
        finished_at: r.get(10)?,
        outcome: r.get(11)?,
        outcome_source: r.get(12)?,
        error_code: r.get(13)?,
        host_alias: r.get(14)?,
    })
}

impl Store {
    /// Insert a routine; `next_run_at` and `event_cursor` are the
    /// scheduler's starting state, computed by the service.
    pub fn insert_routine(
        &self,
        org_id: Option<i64>,
        owner_person_id: Option<i64>,
        f: &RoutineFields,
        next_run_at: Option<i64>,
        event_cursor: i64,
    ) -> Result<RoutineRow, IpcError> {
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO routines (org_id, owner_person_id, name, enabled, trigger, cron, \
               utc_offset_min, event, event_cursor, host_alias, project_id, profile, prompt, \
               budget_run_micros, budget_day_micros, overlap, next_run_at, created_at, updated_at, \
               event_repo, event_author, event_rate_secs, time_zone, run_max_secs, fallback_host, \
               retry_once, autonomy) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18, \
               ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            rusqlite::params![
                org_id,
                owner_person_id,
                f.name,
                f.enabled as i64,
                f.trigger,
                f.cron,
                f.utc_offset_min,
                f.event,
                event_cursor,
                f.host_alias,
                f.project_id,
                f.profile,
                f.prompt,
                f.budget_run_micros,
                f.budget_day_micros,
                f.overlap,
                next_run_at,
                now,
                f.event_repo,
                f.event_author,
                f.event_rate_secs,
                f.time_zone,
                f.run_max_secs,
                f.fallback_host,
                f.retry_once as i64,
                f.autonomy
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.get_routine(id)?.expect("the row just inserted"))
    }

    /// Replace a routine's fields. A change clears `skip_next` and
    /// `paused_reason`: the schedule and the reason it stopped start over.
    pub fn update_routine(
        &self,
        id: i64,
        f: &RoutineFields,
        next_run_at: Option<i64>,
        event_cursor: i64,
    ) -> Result<Option<RoutineRow>, IpcError> {
        self.conn.execute(
            "UPDATE routines SET name = ?1, enabled = ?2, trigger = ?3, cron = ?4, \
               utc_offset_min = ?5, event = ?6, event_cursor = ?7, host_alias = ?8, \
               project_id = ?9, profile = ?10, prompt = ?11, budget_run_micros = ?12, \
               budget_day_micros = ?13, overlap = ?14, next_run_at = ?15, skip_next = 0, \
               paused_reason = NULL, updated_at = ?16, event_repo = ?18, event_author = ?19, \
               event_rate_secs = ?20, time_zone = ?21, run_max_secs = ?22, fallback_host = ?23, \
               retry_once = ?24, autonomy = ?25 \
             WHERE id = ?17",
            rusqlite::params![
                f.name,
                f.enabled as i64,
                f.trigger,
                f.cron,
                f.utc_offset_min,
                f.event,
                event_cursor,
                f.host_alias,
                f.project_id,
                f.profile,
                f.prompt,
                f.budget_run_micros,
                f.budget_day_micros,
                f.overlap,
                next_run_at,
                now_unix(),
                id,
                f.event_repo,
                f.event_author,
                f.event_rate_secs,
                f.time_zone,
                f.run_max_secs,
                f.fallback_host,
                f.retry_once as i64,
                f.autonomy
            ],
        )?;
        self.get_routine(id)
    }

    pub fn get_routine(&self, id: i64) -> Result<Option<RoutineRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM routines WHERE id = ?1"),
                [id],
                routine,
            )
            .optional()?)
    }

    /// Every routine, by name.
    pub fn list_routines(&self) -> Result<Vec<RoutineRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM routines ORDER BY name COLLATE NOCASE, id"
        ))?;
        let rows = stmt.query_map([], routine)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Remove a routine and its runs. `false`: no such routine.
    pub fn delete_routine(&self, id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .execute("DELETE FROM routines WHERE id = ?1", [id])?
            == 1)
    }

    /// Turn a routine on or off. On: the schedule restarts at `next_run_at`
    /// and the pause reason goes. Off: `reason` says why when the scheduler
    /// did it (`None` for a person's switch).
    pub fn set_routine_enabled(
        &self,
        id: i64,
        enabled: bool,
        reason: Option<&str>,
        next_run_at: Option<i64>,
    ) -> Result<Option<RoutineRow>, IpcError> {
        self.conn.execute(
            "UPDATE routines SET enabled = ?1, paused_reason = ?2, next_run_at = ?3, \
               updated_at = ?4 WHERE id = ?5",
            rusqlite::params![
                enabled as i64,
                if enabled { None } else { reason },
                next_run_at,
                now_unix(),
                id
            ],
        )?;
        self.get_routine(id)
    }

    /// Mark (or unmark) the next scheduled fire as skipped.
    pub fn set_routine_skip_next(
        &self,
        id: i64,
        skip: bool,
    ) -> Result<Option<RoutineRow>, IpcError> {
        self.conn.execute(
            "UPDATE routines SET skip_next = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![skip as i64, now_unix(), id],
        )?;
        self.get_routine(id)
    }

    /// The enabled cron routines whose fire is due at `now`, oldest first.
    pub fn routines_due(&self, now: i64) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM routines WHERE enabled = 1 AND trigger = 'cron' \
               AND next_run_at IS NOT NULL AND next_run_at <= ?1 \
               AND (lease_until IS NULL OR lease_until < ?1) \
             ORDER BY next_run_at, id",
        )?;
        let rows = stmt.query_map([now], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
    }

    /// The enabled event routines.
    pub fn event_routines(&self) -> Result<Vec<RoutineRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM routines WHERE enabled = 1 AND trigger = 'event' ORDER BY id"
        ))?;
        let rows = stmt.query_map([], routine)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Take the routine for one tick. `false`: another tick holds it.
    pub fn take_routine_lease(&self, id: i64, now: i64) -> Result<bool, IpcError> {
        Ok(self.conn.execute(
            "UPDATE routines SET lease_until = ?1 \
             WHERE id = ?2 AND (lease_until IS NULL OR lease_until < ?3)",
            rusqlite::params![now + ROUTINE_LEASE_SECS, id, now],
        )? == 1)
    }

    pub fn release_routine_lease(&self, id: i64) -> Result<(), IpcError> {
        self.conn
            .execute("UPDATE routines SET lease_until = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The scheduler moved on: the next fire, and `skip_next` cleared when
    /// this fire spent it. A Skip next set while the fire ran stays for the
    /// next one (review r06 F3).
    pub fn advance_routine(
        &self,
        id: i64,
        next_run_at: Option<i64>,
        skip_spent: bool,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routines SET next_run_at = ?1, \
               skip_next = CASE WHEN ?3 THEN 0 ELSE skip_next END WHERE id = ?2",
            rusqlite::params![next_run_at, id, skip_spent],
        )?;
        Ok(())
    }

    pub fn set_routine_event_cursor(&self, id: i64, cursor: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routines SET event_cursor = ?1 WHERE id = ?2",
            rusqlite::params![cursor, id],
        )?;
        Ok(())
    }

    /// The newest session event's id (0 for none): where a new event
    /// routine starts looking.
    pub fn latest_session_event_id(&self) -> Result<i64, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM session_events", [], |r| {
                r.get(0)
            })?)
    }

    /// Session events of `kind` after `cursor`, oldest first, at most
    /// `limit`: `(event id, session id, detail)`.
    pub fn events_of_kind_after(
        &self,
        cursor: i64,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<(i64, i64, Option<String>)>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, session_id, detail FROM session_events \
             WHERE id > ?1 AND kind = ?2 ORDER BY id LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor, kind, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Whether a run of routine `id` that was not skipped fired on
    /// `subject` (a `trigger_ref` prefix such as `pr:<id>:`) at or after
    /// `since`: the event rate (migration 156).
    pub fn routine_fired_on_since(
        &self,
        id: i64,
        subject: &str,
        since: i64,
    ) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM routine_runs WHERE routine_id = ?1 AND started_at >= ?3 \
                   AND state <> 'skipped' \
                   AND substr(trigger_ref, 1, length(?2)) = ?2 LIMIT 1",
                rusqlite::params![id, subject, since],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Whether `session_id` has an event of `kind` after event `after`.
    pub fn session_event_since(
        &self,
        session_id: i64,
        kind: &str,
        after: i64,
    ) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM session_events WHERE session_id = ?1 AND kind = ?2 AND id > ?3 \
                 LIMIT 1",
                rusqlite::params![session_id, kind, after],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn insert_routine_run(&self, r: &NewRoutineRun<'_>) -> Result<RoutineRunRow, IpcError> {
        let now = r.at;
        let finished = (r.state != "running").then_some(now);
        self.conn.execute(
            // A run that failed before it started is `failed` from its exit
            // (step 8.10); nothing later may say otherwise.
            "INSERT INTO routine_runs (routine_id, trigger, trigger_ref, state, reason, \
               session_id, scheduled_for, started_at, finished_at, outcome, outcome_source, \
               error_code, host_alias) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
               CASE ?4 WHEN 'failed' THEN 'failed' END, CASE ?4 WHEN 'failed' THEN 'exit' END, \
               ?10, ?11)",
            rusqlite::params![
                r.routine_id,
                r.trigger,
                r.trigger_ref,
                r.state,
                r.reason,
                r.session_id,
                r.scheduled_for,
                now,
                finished,
                r.error_code,
                r.host_alias
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.get_routine_run(id)?.expect("the row just inserted"))
    }

    pub fn get_routine_run(&self, id: i64) -> Result<Option<RoutineRunRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM routine_runs WHERE id = ?1"),
                [id],
                run,
            )
            .optional()?)
    }

    /// A routine's runs, newest first.
    pub fn routine_runs(
        &self,
        routine_id: i64,
        limit: i64,
    ) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM routine_runs WHERE routine_id = ?1 \
             ORDER BY started_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![routine_id, limit], run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every open run, oldest first.
    pub fn running_routine_runs(&self) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM routine_runs WHERE state = 'running' ORDER BY id"
        ))?;
        let rows = stmt.query_map([], run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Whether the routine has a run open.
    pub fn routine_run_open(&self, routine_id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM routine_runs WHERE routine_id = ?1 AND state = 'running' LIMIT 1",
                [routine_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// What the routine's runs started at or after `since` spent.
    pub fn routine_cost_since(&self, routine_id: i64, since: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_micros), 0) FROM routine_runs \
             WHERE routine_id = ?1 AND started_at >= ?2",
            rusqlite::params![routine_id, since],
            |r| r.get(0),
        )?)
    }

    /// What every routine's runs started at or after `since` spent: the
    /// fleet's routine spend (`automation.daily_budget`, M15 G3.8).
    pub fn routines_cost_since(&self, since: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_micros), 0) FROM routine_runs WHERE started_at >= ?1",
            [since],
            |r| r.get(0),
        )?)
    }

    /// The failed runs to start again (`retry_once`, M15 G3.8): each is
    /// its routine's newest run, failed at or after `since`, not itself a
    /// retry (`trigger_ref` `retry:<id>`), of a routine that is on and
    /// asks for it. A retry that is skipped or fails is newer than the run
    /// it retried, so a run is retried at most once. Oldest first.
    pub fn routine_runs_to_retry(&self, since: i64) -> Result<Vec<RoutineRunRow>, IpcError> {
        let cols = RUN_COLS
            .split(", ")
            .map(|c| format!("rr.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols} FROM routine_runs rr JOIN routines r ON r.id = rr.routine_id \
             WHERE rr.state = 'failed' AND rr.started_at >= ?1 \
               AND r.enabled = 1 AND r.retry_once = 1 \
               AND (rr.trigger_ref IS NULL OR rr.trigger_ref NOT LIKE 'retry:%') \
               AND NOT EXISTS (SELECT 1 FROM routine_runs n \
                               WHERE n.routine_id = rr.routine_id AND n.id > rr.id) \
             ORDER BY rr.id"
        ))?;
        let rows = stmt.query_map([since], run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Record why a run failed, as a code.
    pub fn set_routine_run_error_code(&self, id: i64, code: &str) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routine_runs SET error_code = ?1 WHERE id = ?2",
            rusqlite::params![code, id],
        )?;
        Ok(())
    }

    /// A running run's cost so far.
    pub fn set_routine_run_cost(&self, id: i64, cost_micros: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routine_runs SET cost_micros = ?1 WHERE id = ?2",
            rusqlite::params![cost_micros, id],
        )?;
        Ok(())
    }

    /// Close a run: `done` or `failed`, with why.
    pub fn finish_routine_run(
        &self,
        id: i64,
        state: &str,
        reason: Option<&str>,
        cost_micros: i64,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routine_runs SET state = ?1, reason = ?2, cost_micros = ?3, finished_at = ?4 \
             WHERE id = ?5 AND state = 'running'",
            rusqlite::params![state, reason, cost_micros, now, id],
        )?;
        Ok(())
    }

    /// Record what a finished run came to (migration 133, step 8.10).
    /// Refuses a value outside [`ROUTINE_RUN_OUTCOMES`] /
    /// [`ROUTINE_RUN_OUTCOME_SOURCES`], and never lets a weaker source
    /// replace a stronger one's answer (a failed exit stands whatever Jev
    /// says). Answers whether the row changed.
    pub fn set_routine_run_outcome(
        &self,
        id: i64,
        outcome: &str,
        source: &str,
    ) -> Result<bool, IpcError> {
        if !ROUTINE_RUN_OUTCOMES.contains(&outcome) {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!(
                    "a run's outcome is one of {}, not {outcome:?}",
                    ROUTINE_RUN_OUTCOMES.join(", ")
                ),
            ));
        }
        let Some(rank) = ROUTINE_RUN_OUTCOME_SOURCES
            .iter()
            .position(|s| *s == source)
        else {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!(
                    "a run's outcome comes from one of {}, not {source:?}",
                    ROUTINE_RUN_OUTCOME_SOURCES.join(", ")
                ),
            ));
        };
        let n = self.conn.execute(
            "UPDATE routine_runs SET outcome = ?1, outcome_source = ?2 \
             WHERE id = ?3 AND state <> 'running' \
               AND (outcome IS NOT ?1 OR outcome_source IS NOT ?2) \
               AND (CASE outcome_source WHEN 'jev' THEN 0 WHEN 'rule' THEN 1 \
                    WHEN 'exit' THEN 2 ELSE -1 END) <= ?4",
            rusqlite::params![outcome, source, id, rank as i64],
        )?;
        Ok(n > 0)
    }

    /// Finished runs nothing has answered yet, oldest first: what Jev's
    /// routine_run_outcome use case reads once it is on.
    pub fn routine_runs_without_outcome(&self, limit: i64) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {RUN_COLS} FROM routine_runs \
             WHERE state = 'done' AND outcome IS NULL ORDER BY id LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit], run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Finished runs nothing has answered yet that finished at `since` or
    /// later, newest first: the runs whose screen Jev can still read.
    pub fn recent_routine_runs_without_outcome(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {RUN_COLS} FROM routine_runs \
             WHERE state = 'done' AND outcome IS NULL AND finished_at >= ?1 \
             ORDER BY finished_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit], run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod orphan_tests {
    use super::*;

    /// A cron routine on `host`, in project `pid`, enabled, due at 100.
    fn routine_on(s: &Store, host: &str, pid: i64) -> i64 {
        s.conn
            .execute(
                "INSERT INTO routines (name, trigger, cron, host_alias, project_id, prompt, \
                   next_run_at, created_at, updated_at) \
                 VALUES ('r', 'cron', '0 9 * * *', ?1, ?2, 'go', 100, 1, 1)",
                rusqlite::params![host, pid],
            )
            .unwrap();
        s.conn.last_insert_rowid()
    }

    fn project(s: &Store, repo: &str) -> i64 {
        s.conn
            .execute(
                "INSERT INTO projects (owner, repo, base_path) VALUES ('me', ?1, '/p/' || ?1)",
                [repo],
            )
            .unwrap();
        s.conn.last_insert_rowid()
    }

    /// Review r02: a routine whose project or host is removed stops with the
    /// reason instead of failing on every fire; a merged host takes its
    /// routines along; and migration 146 stops the ones already orphaned.
    #[test]
    fn a_routine_stops_when_its_project_or_host_goes_and_follows_a_merge() {
        let s = Store::open_in_memory().unwrap();
        for h in ["h1", "h2", "old", "new"] {
            s.upsert_host(h).unwrap();
        }
        let (p1, p2) = (project(&s, "one"), project(&s, "two"));
        let on_project = routine_on(&s, "h1", p1);
        let on_host = routine_on(&s, "h2", p2);
        let merged = routine_on(&s, "old", p2);
        let bystander = routine_on(&s, "h1", p2);

        s.delete_project(p1, &Default::default()).unwrap();
        s.delete_host("h2").unwrap();
        s.merge_host_alias("old", "new").unwrap();

        let r = s.get_routine(on_project).unwrap().unwrap();
        assert!(!r.enabled && r.next_run_at.is_none());
        assert_eq!(r.paused_reason.as_deref(), Some(PAUSED_PROJECT_REMOVED));
        let r = s.get_routine(on_host).unwrap().unwrap();
        assert!(!r.enabled);
        assert_eq!(r.paused_reason.as_deref(), Some(PAUSED_HOST_REMOVED));
        let r = s.get_routine(merged).unwrap().unwrap();
        assert!(r.enabled);
        assert_eq!(r.host_alias, "new");
        let r = s.get_routine(bystander).unwrap().unwrap();
        assert!(r.enabled && r.paused_reason.is_none());
    }

    #[test]
    fn migration_146_stops_routines_orphaned_before_it() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let p = project(&s, "one");
        let live = routine_on(&s, "h1", p);
        let local = routine_on(&s, "local", p);
        let no_project = routine_on(&s, "h1", 9_999);
        let no_host = routine_on(&s, "gone", p);
        s.conn
            .execute("DELETE FROM schema_version WHERE version >= 146", [])
            .unwrap();
        s.migrate().unwrap();
        let reason = |id| s.get_routine(id).unwrap().unwrap().paused_reason;
        assert_eq!(reason(live), None);
        assert_eq!(reason(local), None, "local is never removed");
        assert_eq!(reason(no_project).as_deref(), Some(PAUSED_PROJECT_REMOVED));
        assert_eq!(reason(no_host).as_deref(), Some(PAUSED_HOST_REMOVED));
        assert!(!s.get_routine(no_host).unwrap().unwrap().enabled);
    }

    /// The scheduler's every-tick scan of unjudged runs reads its partial
    /// index, not the whole run history.
    #[test]
    fn the_unjudged_runs_scan_uses_its_index() {
        let s = Store::open_in_memory().unwrap();
        let plan: Vec<String> = {
            let mut stmt = s
                .conn
                .prepare(
                    "EXPLAIN QUERY PLAN SELECT id FROM routine_runs \
                     WHERE state = 'done' AND outcome IS NULL AND finished_at >= ?1 \
                     ORDER BY finished_at DESC, id DESC LIMIT ?2",
                )
                .unwrap();
            let rows = stmt
                .query_map(rusqlite::params![0, 10], |r| r.get::<_, String>(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows
        };
        assert!(
            plan.iter().any(|l| l.contains("routine_runs_unjudged")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|l| l.contains("TEMP B-TREE")), "{plan:?}");
    }
}
