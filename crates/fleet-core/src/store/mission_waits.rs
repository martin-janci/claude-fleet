//! Why a mission's ready work waits (mission improvements M3, design
//! 2026-10-26 §3, migration 214). The loop knows on every tick what held a
//! ready task back; this keeps, per mission and cause, how long some ready
//! work waited on it. A look adds the time since the previous one to the
//! causes that were open then, and opens the ones it sees now, so a cause
//! counts from when it was seen, not before.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use std::collections::BTreeMap;

/// The causes, the one nearer a person first: a task held for two reasons
/// is the first one's (`service::work::orchestrate::waits`).
pub const WAIT_CAUSES: &[&str] = &[
    "person", "grant", "limit", "brake", "slot", "lane", "touches",
];

/// The most one look adds: a hub that slept, or a loop that was off, does
/// not count its gap as waiting. Two idle rechecks.
pub const WAIT_STEP_MAX_SECS: i64 = 2 * 3600;

impl Store {
    /// Look at mission `id`'s waits at `now`: the time since the last look
    /// (at most [`WAIT_STEP_MAX_SECS`]) goes to the causes open then, and
    /// `causes` are the open ones from now on (none: nothing waits).
    pub fn account_mission_waits(
        &self,
        id: i64,
        now: i64,
        causes: &[&str],
    ) -> Result<(), IpcError> {
        if let Some(bad) = causes.iter().find(|c| !WAIT_CAUSES.contains(c)) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("wait cause {bad:?}; one of {}", WAIT_CAUSES.join(", ")),
            ));
        }
        self.in_savepoint("account_mission_waits", |conn| -> Result<(), IpcError> {
            use rusqlite::OptionalExtension;
            let at: Option<i64> = conn
                .query_row(
                    "SELECT waits_at FROM orchestration_projects WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let step = at.map_or(0, |a| (now - a).clamp(0, WAIT_STEP_MAX_SECS));
            if step > 0 {
                conn.execute(
                    "UPDATE mission_waits SET secs = secs + ?2 \
                     WHERE orchestration_project_id = ?1 AND open = 1",
                    rusqlite::params![id, step],
                )?;
            }
            conn.execute(
                "UPDATE mission_waits SET open = 0 \
                 WHERE orchestration_project_id = ?1 AND open = 1",
                [id],
            )?;
            for c in causes {
                conn.execute(
                    "INSERT INTO mission_waits (orchestration_project_id, cause, secs, open) \
                     VALUES (?1, ?2, 0, 1) \
                     ON CONFLICT (orchestration_project_id, cause) DO UPDATE SET open = 1",
                    rusqlite::params![id, c],
                )?;
            }
            conn.execute(
                "UPDATE orchestration_projects SET waits_at = ?2 WHERE id = ?1",
                rusqlite::params![id, now],
            )?;
            Ok(())
        })
    }

    /// How long mission `id`'s ready work waited, by cause, as of `now`: a
    /// cause still open counts until `now` (at most [`WAIT_STEP_MAX_SECS`]
    /// past the last look). Only causes that held something a second or more.
    pub fn mission_waits(&self, id: i64, now: i64) -> Result<BTreeMap<String, i64>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT w.cause, w.secs + CASE WHEN w.open = 1 AND p.waits_at IS NOT NULL \
                    THEN MIN(MAX(?2 - p.waits_at, 0), ?3) ELSE 0 END \
               FROM mission_waits w \
               JOIN orchestration_projects p ON p.id = w.orchestration_project_id \
              WHERE w.orchestration_project_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![id, now, WAIT_STEP_MAX_SECS], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (cause, secs) = row?;
            if secs > 0 {
                out.insert(cause, secs);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission(s: &Store) -> i64 {
        s.create_mission(
            &crate::store::NewMission {
                name: "M",
                goal: "g",
                ..Default::default()
            },
            "test",
        )
        .unwrap()
        .id
    }

    #[test]
    fn a_cause_counts_from_when_it_was_seen_until_the_next_look() {
        let s = Store::open_in_memory().unwrap();
        let m = mission(&s);
        // The first look only opens: nothing before it is counted.
        s.account_mission_waits(m, 1000, &["slot"]).unwrap();
        assert!(s.mission_waits(m, 1000).unwrap().is_empty());
        s.account_mission_waits(m, 1060, &["slot", "person"])
            .unwrap();
        s.account_mission_waits(m, 1100, &[]).unwrap();
        s.account_mission_waits(m, 1500, &["slot"]).unwrap();
        let w = s.mission_waits(m, 1500).unwrap();
        assert_eq!(
            w.get("slot"),
            Some(&100),
            "1000→1100, not the idle 400 after"
        );
        assert_eq!(w.get("person"), Some(&40));
        // A cause still open counts until the read.
        assert_eq!(s.mission_waits(m, 1530).unwrap().get("slot"), Some(&130));
    }

    #[test]
    fn a_long_gap_counts_at_most_two_rechecks() {
        let s = Store::open_in_memory().unwrap();
        let m = mission(&s);
        s.account_mission_waits(m, 0, &["limit"]).unwrap();
        s.account_mission_waits(m, 100_000, &[]).unwrap();
        assert_eq!(
            s.mission_waits(m, 100_000).unwrap()["limit"],
            WAIT_STEP_MAX_SECS
        );
    }

    #[test]
    fn an_unknown_cause_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let m = mission(&s);
        assert!(s.account_mission_waits(m, 1, &["tick"]).is_err());
    }
}
