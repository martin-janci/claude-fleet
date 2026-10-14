//! What the routines' scheduler decided, per routine (migration 186): the
//! trace `routines { action: explain }` reads to answer "why did it (not)
//! fire?". The rules themselves are `service::routines::tick`'s.

use super::Store;
use crate::ipc_error::IpcError;
use serde::{Deserialize, Serialize};

/// `routine_decisions.verdict` values: every one its `CHECK` admits.
pub const ROUTINE_DECISION_VERDICTS: [&str; 5] =
    ["fired", "failed", "skipped", "dropped", "filtered"];
/// The decisions kept per routine, newest first.
pub const ROUTINE_DECISIONS_KEPT: i64 = 200;

/// One decision, as a reader receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineDecisionRow {
    pub id: i64,
    pub routine_id: i64,
    pub at: i64,
    /// cron | event | run_now | retry
    pub trigger: String,
    /// One of [`ROUTINE_DECISION_VERDICTS`].
    pub verdict: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_ref: Option<String>,
    /// The timeline event an event routine read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
    /// The session that event was on, or the run's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// The run it recorded, when it recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

/// A decision to record.
#[derive(Debug, Clone, Default)]
pub struct NewRoutineDecision<'a> {
    pub routine_id: i64,
    pub at: i64,
    pub trigger: &'a str,
    pub verdict: &'a str,
    pub reason: Option<&'a str>,
    pub trigger_ref: Option<&'a str>,
    pub event_id: Option<i64>,
    pub session_id: Option<i64>,
    pub run_id: Option<i64>,
}

/// A session event as a routine reads it: `(event id, session id, at,
/// detail)`.
pub type RecentEvent = (i64, i64, i64, Option<String>);
/// One session event by id: `(session id, kind, at, detail)`.
pub type SessionEventById = (i64, String, i64, Option<String>);

const COLS: &str =
    "id, routine_id, at, trigger, verdict, reason, trigger_ref, event_id, session_id, run_id";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineDecisionRow> {
    Ok(RoutineDecisionRow {
        id: r.get(0)?,
        routine_id: r.get(1)?,
        at: r.get(2)?,
        trigger: r.get(3)?,
        verdict: r.get(4)?,
        reason: r.get(5)?,
        trigger_ref: r.get(6)?,
        event_id: r.get(7)?,
        session_id: r.get(8)?,
        run_id: r.get(9)?,
    })
}

impl Store {
    /// Record a decision, then drop the routine's decisions past the newest
    /// [`ROUTINE_DECISIONS_KEPT`].
    pub fn insert_routine_decision(&self, d: &NewRoutineDecision<'_>) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO routine_decisions \
               (routine_id, at, trigger, verdict, reason, trigger_ref, event_id, session_id, \
                run_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                d.routine_id,
                d.at,
                d.trigger,
                d.verdict,
                d.reason,
                d.trigger_ref,
                d.event_id,
                d.session_id,
                d.run_id
            ],
        )?;
        self.conn.execute(
            "DELETE FROM routine_decisions WHERE routine_id = ?1 AND id <= \
               (SELECT id FROM routine_decisions WHERE routine_id = ?1 \
                ORDER BY id DESC LIMIT 1 OFFSET ?2)",
            rusqlite::params![d.routine_id, ROUTINE_DECISIONS_KEPT],
        )?;
        Ok(())
    }

    /// Routine `id`'s decisions, newest first, at most `limit`.
    pub fn routine_decisions(
        &self,
        id: i64,
        limit: i64,
    ) -> Result<Vec<RoutineDecisionRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM routine_decisions WHERE routine_id = ?1 \
             ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![id, limit], row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The newest session events of `kind`, newest first, at most `limit`:
    /// `(event id, session id, at, detail)`. What an event routine would
    /// read, whatever its cursor.
    pub fn recent_events_of_kind(
        &self,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<RecentEvent>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, session_id, at, detail FROM session_events \
             WHERE kind = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![kind, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Session events of `kind` after event `cursor`: what an event routine
    /// has still to read.
    pub fn count_events_of_kind_after(&self, cursor: i64, kind: &str) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM session_events WHERE id > ?1 AND kind = ?2",
            rusqlite::params![cursor, kind],
            |r| r.get(0),
        )?)
    }

    /// The session event `id`: `(session id, kind, at, detail)`.
    pub fn session_event_by_id(&self, id: i64) -> Result<Option<SessionEventById>, IpcError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT session_id, kind, at, detail FROM session_events WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(s: &Store) -> i64 {
        s.conn
            .execute(
                "INSERT OR IGNORE INTO projects (id, owner, repo, base_path) \
                 VALUES (1, 'me', 'one', '/p/one')",
                [],
            )
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO routines (name, trigger, host_alias, project_id, prompt, \
                   created_at, updated_at) \
                 VALUES ('r', 'manual', 'local', 1, 'go', 1, 1)",
                [],
            )
            .unwrap();
        s.conn.last_insert_rowid()
    }

    fn decide(s: &Store, id: i64, at: i64, verdict: &str) {
        s.insert_routine_decision(&NewRoutineDecision {
            routine_id: id,
            at,
            trigger: "event",
            verdict,
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn decisions_read_newest_first_and_keep_the_newest_only() {
        let s = Store::open_in_memory().unwrap();
        let a = routine(&s);
        let b = routine(&s);
        decide(&s, b, 1, "fired");
        for at in 0..ROUTINE_DECISIONS_KEPT + 5 {
            decide(&s, a, at, "filtered");
        }
        let rows = s.routine_decisions(a, 1_000).unwrap();
        assert_eq!(rows.len() as i64, ROUTINE_DECISIONS_KEPT);
        assert_eq!(rows[0].at, ROUTINE_DECISIONS_KEPT + 4, "newest first");
        assert_eq!(rows.last().unwrap().at, 5, "the oldest five went");
        assert_eq!(s.routine_decisions(b, 10).unwrap().len(), 1, "per routine");
    }

    #[test]
    fn every_verdict_passes_the_check() {
        let s = Store::open_in_memory().unwrap();
        let a = routine(&s);
        for v in ROUTINE_DECISION_VERDICTS {
            decide(&s, a, 1, v);
        }
        assert!(s
            .insert_routine_decision(&NewRoutineDecision {
                routine_id: a,
                trigger: "cron",
                verdict: "maybe",
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn a_deleted_routine_takes_its_decisions() {
        let s = Store::open_in_memory().unwrap();
        let a = routine(&s);
        decide(&s, a, 1, "fired");
        s.delete_routine(a).unwrap();
        assert!(s.routine_decisions(a, 10).unwrap().is_empty());
    }
}
