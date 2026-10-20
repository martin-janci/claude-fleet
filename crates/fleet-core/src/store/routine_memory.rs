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

    /// The finished runs whose reply was not read for their routine's
    /// notebooks yet, finished at or after `since`, oldest first, at most
    /// `limit`. Only runs of a routine that keeps a notebook.
    pub fn routine_runs_memory_unread(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<RoutineRunRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM routine_runs rr \
             JOIN routines r ON r.id = rr.routine_id \
             WHERE rr.state IN ('done', 'failed') AND rr.memory_read_at IS NULL \
               AND rr.finished_at >= ?1 AND r.memory IS NOT NULL \
             ORDER BY rr.finished_at, rr.id LIMIT ?2",
            super::routines::RUN_COLS_RR
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit], super::routines::run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
