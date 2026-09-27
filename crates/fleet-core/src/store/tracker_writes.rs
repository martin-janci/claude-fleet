//! The write-back outbox (work graph M13.4e, decision D3 / D29; migration
//! 061).
//!
//! A row is a write fleet owes a tracker. [`Store::enqueue_tracker_write`]
//! is idempotent on `(tracker_id, op, item_key, url)`: the same PR seen
//! twice queues once, and a row that is `done` is never queued again. The
//! sync pass takes the due rows ([`Store::due_tracker_writes`]) and settles
//! each ([`Store::finish_tracker_write`], [`Store::retry_tracker_write`]);
//! after [`WRITE_MAX_ATTEMPTS`] a row stays `failed` and counts in
//! `fleet_health`. Settled rows go once they are older than the journal's
//! retention window ([`Store::sweep_tracker_writes`]).

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// The one op today: a PR as a remote link on the item.
pub const WRITE_OP_PR_REMOTE_LINK: &str = "pr_remote_link";

/// Attempts before a write is given up (`failed`).
pub const WRITE_MAX_ATTEMPTS: i64 = 5;

/// One outbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerWriteRow {
    pub id: i64,
    pub tracker_id: i64,
    pub item_key: String,
    pub op: String,
    pub url: String,
    pub title: String,
    pub link_id: Option<i64>,
    pub claude_session_id: Option<String>,
    pub session_org_id: Option<i64>,
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub next_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub done_at: Option<i64>,
}

/// What [`Store::enqueue_tracker_write`] needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrackerWrite<'a> {
    pub tracker_id: i64,
    pub item_key: &'a str,
    pub op: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub link_id: Option<i64>,
    pub claude_session_id: Option<&'a str>,
    pub session_org_id: Option<i64>,
}

const COLUMNS: &str = "id, tracker_id, item_key, op, url, title, link_id, claude_session_id, \
     session_org_id, state, attempts, last_error, next_at, created_at, updated_at, done_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackerWriteRow> {
    Ok(TrackerWriteRow {
        id: r.get(0)?,
        tracker_id: r.get(1)?,
        item_key: r.get(2)?,
        op: r.get(3)?,
        url: r.get(4)?,
        title: r.get(5)?,
        link_id: r.get(6)?,
        claude_session_id: r.get(7)?,
        session_org_id: r.get(8)?,
        state: r.get(9)?,
        attempts: r.get(10)?,
        last_error: r.get(11)?,
        next_at: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
        done_at: r.get(15)?,
    })
}

impl Store {
    /// Queue a write, due now. `true` when a row was added; `false` when the
    /// same write is already queued, done or given up.
    pub fn enqueue_tracker_write(&self, w: &NewTrackerWrite<'_>) -> Result<bool, IpcError> {
        let now = now_unix();
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO tracker_writes \
               (tracker_id, item_key, op, url, title, link_id, claude_session_id, \
                session_org_id, state, attempts, next_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, ?9, ?9, ?9)",
            rusqlite::params![
                w.tracker_id,
                w.item_key,
                w.op,
                w.url,
                w.title,
                w.link_id,
                w.claude_session_id,
                w.session_org_id,
                now
            ],
        )?;
        Ok(n > 0)
    }

    /// A tracker's pending writes that are due at `now`, oldest first.
    pub fn due_tracker_writes(
        &self,
        tracker_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<TrackerWriteRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM tracker_writes \
             WHERE tracker_id = ?1 AND state = 'pending' AND next_at <= ?2 \
             ORDER BY next_at, id LIMIT ?3"
        ))?;
        let rows = stmt.query_map(rusqlite::params![tracker_id, now, limit as i64], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One row by id.
    pub fn tracker_write(&self, id: i64) -> Result<Option<TrackerWriteRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM tracker_writes WHERE id = ?1"),
                [id],
                map_row,
            )
            .optional()?)
    }

    /// The write landed.
    pub fn finish_tracker_write(&self, id: i64) -> Result<(), IpcError> {
        let now = now_unix();
        self.conn.execute(
            "UPDATE tracker_writes SET state = 'done', attempts = attempts + 1, \
               last_error = NULL, done_at = ?2, updated_at = ?2 \
             WHERE id = ?1 AND state = 'pending'",
            rusqlite::params![id, now],
        )?;
        Ok(())
    }

    /// The write failed with `error`. `next_at` is when to try again, or
    /// `None` to give up now (a refusal no retry can fix). The last allowed
    /// attempt gives up too. `counts` is false for a failure that is not the
    /// write's fault (a rate limit): the attempt is not spent.
    pub fn retry_tracker_write(
        &self,
        id: i64,
        error: &str,
        next_at: Option<i64>,
        counts: bool,
    ) -> Result<(), IpcError> {
        let now = now_unix();
        let error: String = error.chars().take(300).collect();
        self.conn.execute(
            "UPDATE tracker_writes SET \
               attempts = attempts + ?4, \
               last_error = ?2, \
               state = CASE WHEN ?3 IS NULL OR attempts + ?4 >= ?5 THEN 'failed' ELSE 'pending' END, \
               next_at = COALESCE(?3, next_at), \
               updated_at = ?6 \
             WHERE id = ?1 AND state = 'pending'",
            rusqlite::params![
                id,
                error,
                next_at,
                i64::from(counts),
                WRITE_MAX_ATTEMPTS,
                now
            ],
        )?;
        Ok(())
    }

    /// Writes a tracker has given up on (`fleet_health`'s `write_failures`).
    pub fn tracker_write_failures(&self, tracker_id: i64) -> Result<u64, IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tracker_writes WHERE tracker_id = ?1 AND state = 'failed'",
            [tracker_id],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    }

    /// Drop at most `limit` settled rows (`done` / `failed`) last touched
    /// before `cutoff`; `pending` rows are never swept. A done row that goes
    /// lets the same PR queue again; the tracker then upserts the same
    /// remote link, so that costs one call and changes nothing. A failed row
    /// is kept for the whole window, so `fleet_health`'s `write_failures`
    /// still counts it. Returns how many went. Run by the GC retention sweep
    /// (M12.3), one batch per store lock.
    pub fn sweep_tracker_writes(&self, cutoff: i64, limit: usize) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM tracker_writes WHERE id IN (\
               SELECT id FROM tracker_writes \
               WHERE state IN ('done', 'failed') AND updated_at < ?1 \
               ORDER BY id LIMIT ?2)",
            rusqlite::params![cutoff, limit as i64],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_tracker() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "J", "https://acme.atlassian.net")
            .unwrap();
        (s, t.id)
    }

    fn pr(tracker_id: i64, url: &str) -> NewTrackerWrite<'_> {
        NewTrackerWrite {
            tracker_id,
            item_key: "ABC-1",
            op: WRITE_OP_PR_REMOTE_LINK,
            url,
            title: "PR: o/r#1",
            link_id: Some(1),
            claude_session_id: None,
            session_org_id: None,
        }
    }

    #[test]
    fn the_same_write_queues_once_even_after_it_is_done() {
        let (s, t) = store_with_tracker();
        let url = "https://github.com/o/r/pull/1";
        assert!(s.enqueue_tracker_write(&pr(t, url)).unwrap());
        assert!(!s.enqueue_tracker_write(&pr(t, url)).unwrap());
        let due = s.due_tracker_writes(t, now_unix() + 1, 10).unwrap();
        assert_eq!(due.len(), 1);
        s.finish_tracker_write(due[0].id).unwrap();
        assert!(!s.enqueue_tracker_write(&pr(t, url)).unwrap());
        assert!(s
            .due_tracker_writes(t, now_unix() + 1, 10)
            .unwrap()
            .is_empty());
        let row = s.tracker_write(due[0].id).unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.attempts), ("done", 1));
        assert!(row.done_at.is_some());
    }

    #[test]
    fn a_write_retries_then_gives_up_and_a_rate_limit_spends_no_attempt() {
        let (s, t) = store_with_tracker();
        s.enqueue_tracker_write(&pr(t, "https://github.com/o/r/pull/2"))
            .unwrap();
        let id = s.due_tracker_writes(t, now_unix() + 1, 10).unwrap()[0].id;
        let later = now_unix() + 600;
        s.retry_tracker_write(id, "rate limited", Some(later), false)
            .unwrap();
        let row = s.tracker_write(id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.attempts, row.next_at),
            ("pending", 0, later)
        );
        assert!(s
            .due_tracker_writes(t, now_unix() + 1, 10)
            .unwrap()
            .is_empty());
        for _ in 0..(WRITE_MAX_ATTEMPTS - 1) {
            s.retry_tracker_write(id, "503", Some(now_unix()), true)
                .unwrap();
        }
        assert_eq!(s.tracker_write(id).unwrap().unwrap().state, "pending");
        s.retry_tracker_write(id, "503 again", Some(now_unix()), true)
            .unwrap();
        let row = s.tracker_write(id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.attempts),
            ("failed", WRITE_MAX_ATTEMPTS)
        );
        assert_eq!(row.last_error.as_deref(), Some("503 again"));
        assert_eq!(s.tracker_write_failures(t).unwrap(), 1);
    }

    #[test]
    fn a_refusal_gives_up_at_once() {
        let (s, t) = store_with_tracker();
        s.enqueue_tracker_write(&pr(t, "https://github.com/o/r/pull/3"))
            .unwrap();
        let id = s.due_tracker_writes(t, now_unix() + 1, 10).unwrap()[0].id;
        s.retry_tracker_write(id, "forbidden", None, true).unwrap();
        assert_eq!(s.tracker_write(id).unwrap().unwrap().state, "failed");
    }

    #[test]
    fn settled_rows_are_swept_and_pending_ones_kept() {
        let (s, t) = store_with_tracker();
        for n in 1..=3 {
            s.enqueue_tracker_write(&pr(t, &format!("https://github.com/o/r/pull/{n}")))
                .unwrap();
        }
        let due = s.due_tracker_writes(t, now_unix() + 1, 10).unwrap();
        s.finish_tracker_write(due[0].id).unwrap();
        s.retry_tracker_write(due[1].id, "no", None, true).unwrap();
        assert_eq!(s.sweep_tracker_writes(now_unix() + 10, 1).unwrap(), 1);
        assert_eq!(s.sweep_tracker_writes(now_unix() + 10, 100).unwrap(), 1);
        assert_eq!(
            s.due_tracker_writes(t, now_unix() + 1, 10).unwrap().len(),
            1
        );
    }

    #[test]
    fn removing_the_tracker_removes_its_writes() {
        let (s, t) = store_with_tracker();
        s.enqueue_tracker_write(&pr(t, "https://github.com/o/r/pull/4"))
            .unwrap();
        s.remove_tracker(t).unwrap();
        let n: i64 = s
            .conn_ref()
            .query_row("SELECT COUNT(*) FROM tracker_writes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
