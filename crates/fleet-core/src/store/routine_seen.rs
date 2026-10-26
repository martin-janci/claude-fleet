//! Routine seen items (migration 212): what a routine's runs already
//! handled, item by item, with the state each run saw. What a run is shown
//! and how its reply updates them are `service::routines::seen`'s; this is
//! the rows.

use super::Store;
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// One item a routine's runs handled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineSeenRow {
    /// Its stable name: `pr owner/repo#12`.
    pub item: String,
    /// What the run that reported it last saw.
    pub state: String,
    pub first_seen_at: i64,
    /// The last run that reported it, and when.
    pub seen_at: i64,
    /// When its state last differed from the one before.
    pub changed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

/// What a report did to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenChange {
    /// No run had reported it.
    New,
    /// Its state differs from the one last reported.
    Changed,
    /// Reported as it was.
    Same,
}

fn seen(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineSeenRow> {
    Ok(RoutineSeenRow {
        item: r.get(0)?,
        state: r.get(1)?,
        first_seen_at: r.get(2)?,
        seen_at: r.get(3)?,
        changed_at: r.get(4)?,
        run_id: r.get(5)?,
    })
}

const COLS: &str = "item, state, first_seen_at, seen_at, changed_at, run_id";

impl Store {
    /// Routine `routine_id`'s items, the last seen first, at most `limit`.
    pub fn routine_seen(
        &self,
        routine_id: i64,
        limit: i64,
    ) -> Result<Vec<RoutineSeenRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM routine_seen WHERE routine_id = ?1 \
             ORDER BY seen_at DESC, item LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![routine_id, limit], seen)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many items routine `routine_id` keeps.
    pub fn routine_seen_count(&self, routine_id: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM routine_seen WHERE routine_id = ?1",
            [routine_id],
            |r| r.get(0),
        )?)
    }

    /// Record that run `run_id` of routine `routine_id` handled `item` in
    /// `state` at `now`, and say what that changed.
    pub fn report_routine_seen(
        &self,
        routine_id: i64,
        item: &str,
        state: &str,
        run_id: Option<i64>,
        now: i64,
    ) -> Result<SeenChange, IpcError> {
        let before: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM routine_seen WHERE routine_id = ?1 AND item = ?2",
                rusqlite::params![routine_id, item],
                |r| r.get(0),
            )
            .optional()?;
        let change = match before.as_deref() {
            None => SeenChange::New,
            Some(b) if b == state => SeenChange::Same,
            Some(_) => SeenChange::Changed,
        };
        self.conn.execute(
            "INSERT INTO routine_seen \
               (routine_id, item, state, first_seen_at, seen_at, changed_at, run_id) \
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5) \
             ON CONFLICT (routine_id, item) DO UPDATE SET \
               changed_at = CASE WHEN state IS excluded.state THEN changed_at ELSE ?4 END, \
               state = excluded.state, seen_at = ?4, run_id = ?5",
            rusqlite::params![routine_id, item, state, now, run_id],
        )?;
        Ok(change)
    }

    /// Drop routine `routine_id`'s items no run reported since `before`,
    /// then all but the `keep` last seen. Answers how many went.
    pub fn prune_routine_seen(
        &self,
        routine_id: i64,
        before: i64,
        keep: i64,
    ) -> Result<usize, IpcError> {
        let old = self.conn.execute(
            "DELETE FROM routine_seen WHERE routine_id = ?1 AND seen_at < ?2",
            rusqlite::params![routine_id, before],
        )?;
        let over = self.conn.execute(
            "DELETE FROM routine_seen WHERE routine_id = ?1 AND item NOT IN \
               (SELECT item FROM routine_seen WHERE routine_id = ?1 \
                ORDER BY seen_at DESC, item LIMIT ?2)",
            rusqlite::params![routine_id, keep],
        )?;
        Ok(old + over)
    }

    /// Forget routine `routine_id`'s item `item`, or every item with
    /// `None`. Answers how many went.
    pub fn forget_routine_seen(
        &self,
        routine_id: i64,
        item: Option<&str>,
    ) -> Result<usize, IpcError> {
        Ok(match item {
            Some(i) => self.conn.execute(
                "DELETE FROM routine_seen WHERE routine_id = ?1 AND item = ?2",
                rusqlite::params![routine_id, i],
            )?,
            None => self.conn.execute(
                "DELETE FROM routine_seen WHERE routine_id = ?1",
                [routine_id],
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_says_whether_the_item_is_new_changed_or_the_same() {
        let s = Store::open_in_memory().unwrap();
        let item = "pr o/r#12";
        assert_eq!(
            s.report_routine_seen(7, item, "head=a", Some(1), 100)
                .unwrap(),
            SeenChange::New
        );
        assert_eq!(
            s.report_routine_seen(7, item, "head=a", Some(2), 200)
                .unwrap(),
            SeenChange::Same
        );
        let row = s.routine_seen(7, 10).unwrap().remove(0);
        assert_eq!(
            (row.first_seen_at, row.seen_at, row.changed_at, row.run_id),
            (100, 200, 100, Some(2))
        );
        assert_eq!(
            s.report_routine_seen(7, item, "head=b", Some(3), 300)
                .unwrap(),
            SeenChange::Changed
        );
        let row = s.routine_seen(7, 10).unwrap().remove(0);
        assert_eq!((row.state.as_str(), row.changed_at), ("head=b", 300));
        // Another routine's items are its own.
        assert!(s.routine_seen(8, 10).unwrap().is_empty());
    }

    #[test]
    fn pruning_drops_the_old_then_the_overflow() {
        let s = Store::open_in_memory().unwrap();
        for (i, at) in [10, 20, 30, 40].into_iter().enumerate() {
            s.report_routine_seen(1, &format!("pr o/r#{i}"), "", None, at)
                .unwrap();
        }
        s.report_routine_seen(2, "pr o/r#0", "", None, 10).unwrap();
        assert_eq!(s.prune_routine_seen(1, 15, 2).unwrap(), 2);
        let left: Vec<_> = s
            .routine_seen(1, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.item)
            .collect();
        assert_eq!(left, ["pr o/r#3", "pr o/r#2"]);
        assert_eq!(s.routine_seen_count(2).unwrap(), 1, "routine 2 untouched");
        assert_eq!(s.forget_routine_seen(1, Some("pr o/r#3")).unwrap(), 1);
        assert_eq!(s.forget_routine_seen(1, None).unwrap(), 1);
        assert_eq!(s.routine_seen_count(1).unwrap(), 0);
    }
}
