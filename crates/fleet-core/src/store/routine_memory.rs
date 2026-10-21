//! Routine memory (migration 204): the notebooks the fleet keeps for
//! routines, one per routine and one per project. Who may read and write
//! them, what a run is shown and how its reply rewrites one are
//! `service::routines::memory`'s; this is the rows.

use super::{RoutineRunRow, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// `routine_memory.scope` values.
pub const ROUTINE_MEMORY_SCOPES: [&str; 2] = ["routine", "project"];
/// `routine_memory.updated_by` values.
pub const ROUTINE_MEMORY_WRITERS: [&str; 3] = ["person", "run", "fleet"];
/// `routines.memory` values: which notebooks a routine's runs use.
pub const ROUTINE_MEMORY_MODES: [&str; 3] = ["routine", "project", "both"];

/// One notebook, as a reader receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineMemoryRow {
    /// routine | project
    pub scope: String,
    /// The routine's id, or the project's.
    pub owner_id: i64,
    /// Whose it is (migration 204): an org id, minus a person id, or 0.
    #[serde(default)]
    pub circle: i64,
    pub body: String,
    pub updated_at: i64,
    /// person | run | fleet
    pub updated_by: String,
    /// The run that wrote it last, for `run` and `fleet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_run_id: Option<i64>,
}

fn memory(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineMemoryRow> {
    Ok(RoutineMemoryRow {
        scope: r.get(0)?,
        owner_id: r.get(1)?,
        circle: r.get(2)?,
        body: r.get(3)?,
        updated_at: r.get(4)?,
        updated_by: r.get(5)?,
        updated_run_id: r.get(6)?,
    })
}

/// [`Store::routine_runs_memory_unread`]'s statement: `?1` since, `?2`
/// limit, `?3` now. Its first two predicates are the partial index
/// `routine_runs_memory_unread`'s own (migration 205), word for word, so
/// the planner takes it.
fn memory_unread_sql() -> String {
    format!(
        "SELECT {} FROM routine_runs rr \
         JOIN routines r ON r.id = rr.routine_id \
         WHERE rr.memory_read_at IS NULL AND rr.state IN ('done', 'failed') \
           AND rr.finished_at >= ?1 \
           AND (rr.memory_retry_at IS NULL OR rr.memory_retry_at <= ?3) \
           AND r.memory IS NOT NULL \
         ORDER BY rr.finished_at, rr.id LIMIT ?2",
        super::routines::RUN_COLS_RR
    )
}

fn check(scope: &str, by: &str) -> Result<(), IpcError> {
    if !ROUTINE_MEMORY_SCOPES.contains(&scope) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a notebook is routine or project, not {scope:?}"),
        ));
    }
    if !ROUTINE_MEMORY_WRITERS.contains(&by) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a notebook is written by a person, a run or the fleet, not {by:?}"),
        ));
    }
    Ok(())
}

impl Store {
    /// The notebook of `scope` `owner_id` in `circle`, if anything was
    /// ever written.
    pub fn get_routine_memory(
        &self,
        scope: &str,
        owner_id: i64,
        circle: i64,
    ) -> Result<Option<RoutineMemoryRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT scope, owner_id, circle, body, updated_at, updated_by, updated_run_id \
                 FROM routine_memory WHERE scope = ?1 AND owner_id = ?2 AND circle = ?3",
                rusqlite::params![scope, owner_id, circle],
                memory,
            )
            .optional()?)
    }

    /// Replace the text of notebook `scope` `(owner_id, circle)`. The caps
    /// are the service's. Answers
    /// whether the text changed: writing the same text again touches
    /// nothing, so a reply read twice is not two writes.
    pub fn set_routine_memory(
        &self,
        scope: &str,
        (owner_id, circle): (i64, i64),
        body: &str,
        by: &str,
        run_id: Option<i64>,
        now: i64,
    ) -> Result<bool, IpcError> {
        check(scope, by)?;
        let n = self.conn.execute(
            "INSERT INTO routine_memory (scope, owner_id, circle, body, updated_at, \
               updated_by, updated_run_id) VALUES (?1, ?2, ?7, ?3, ?4, ?5, ?6) \
             ON CONFLICT (scope, owner_id, circle) DO UPDATE SET body = excluded.body, \
               updated_at = excluded.updated_at, updated_by = excluded.updated_by, \
               updated_run_id = excluded.updated_run_id \
             WHERE routine_memory.body IS NOT excluded.body",
            rusqlite::params![scope, owner_id, body, now, by, run_id, circle],
        )?;
        Ok(n > 0)
    }

    /// Replace the text of an existing notebook and nothing else: its
    /// last writer (`updated_at`, `updated_by`, `updated_run_id`) stays,
    /// as an older run's note on a newer notebook needs. Answers whether
    /// the text changed.
    pub fn set_routine_memory_body(
        &self,
        scope: &str,
        (owner_id, circle): (i64, i64),
        body: &str,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE routine_memory SET body = ?4 \
             WHERE scope = ?1 AND owner_id = ?2 AND circle = ?3 AND body IS NOT ?4",
            rusqlite::params![scope, owner_id, circle, body],
        )?;
        Ok(n > 0)
    }

    /// The finished runs whose reply was not read for their routine's
    /// notebooks yet, finished at or after `since` and not held back past
    /// `now` (`memory_retry_at`, migration 205), oldest first, at most
    /// `limit`. Only runs of a routine that keeps a notebook.
    pub fn routine_runs_memory_unread(
        &self,
        since: i64,
        now: i64,
        limit: i64,
    ) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare(&memory_unread_sql())?;
        let rows = stmt.query_map(rusqlite::params![since, limit, now], super::routines::run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Hold run `id` back from the reads until `at`: its transcript could
    /// not be read.
    pub fn hold_routine_run_memory_read(&self, id: i64, at: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routine_runs SET memory_retry_at = ?1 WHERE id = ?2",
            rusqlite::params![at, id],
        )?;
        Ok(())
    }

    /// When run `id` finished, if it is a finished run that still exists.
    pub fn routine_run_finished_at(&self, id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT finished_at FROM routine_runs WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// When run `id`'s reply was read for the notebooks (tests).
    #[cfg(test)]
    pub fn routine_run_memory_read_at(&self, id: i64) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT memory_read_at FROM routine_runs WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .ok()
            .flatten()
    }

    /// Mark run `id`'s reply read for the notebooks.
    pub fn mark_routine_run_memory_read(&self, id: i64, now: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE routine_runs SET memory_read_at = ?1 WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 456: the per-tick probe walks the unread runs' index, not
    /// the runs' whole history.
    #[test]
    fn the_unread_runs_probe_uses_its_index() {
        let s = Store::open_in_memory().unwrap();
        let mut st = s
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", memory_unread_sql()))
            .unwrap();
        let plan = st
            .query_map(rusqlite::params![1, 2, 3], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            plan.contains("routine_runs_memory_unread"),
            "the probe scans: {plan}"
        );
        assert!(!plan.contains("SCAN rr"), "the probe scans: {plan}");
    }
}
