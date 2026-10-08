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
}

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
}

const COLS: &str = "id, org_id, owner_person_id, name, enabled, trigger, cron, utc_offset_min, \
                    event, event_cursor, host_alias, project_id, profile, prompt, \
                    budget_run_micros, budget_day_micros, overlap, next_run_at, skip_next, \
                    paused_reason, created_at, updated_at";

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
    })
}

const RUN_COLS: &str = "id, routine_id, trigger, trigger_ref, state, reason, session_id, \
                        cost_micros, scheduled_for, started_at, finished_at";

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
               budget_run_micros, budget_day_micros, overlap, next_run_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)",
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
                now
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
               paused_reason = NULL, updated_at = ?16 \
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
                id
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

    /// The scheduler moved on: the next fire, and `skip_next` spent.
    pub fn advance_routine(&self, id: i64, next_run_at: Option<i64>) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routines SET next_run_at = ?1, skip_next = 0 WHERE id = ?2",
            rusqlite::params![next_run_at, id],
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
    /// `limit`: `(event id, session id)`.
    pub fn events_of_kind_after(
        &self,
        cursor: i64,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<(i64, i64)>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, session_id FROM session_events \
             WHERE id > ?1 AND kind = ?2 ORDER BY id LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor, kind, limit], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
            "INSERT INTO routine_runs (routine_id, trigger, trigger_ref, state, reason, \
               session_id, scheduled_for, started_at, finished_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                r.routine_id,
                r.trigger,
                r.trigger_ref,
                r.state,
                r.reason,
                r.session_id,
                r.scheduled_for,
                now,
                finished
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
}
