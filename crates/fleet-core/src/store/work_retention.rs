//! Work graph retention (M12.3): bounded growth for the work journal,
//! done tracker items and the work-graph timeline events.
//!
//! A row goes only when ALL hold: it is ended or done, it is older than its
//! window, and nothing live points at it. Each table's rule is ONE CTE
//! (`eligible`), shared by the dry-run count and the delete, so the preview
//! and the sweep cannot disagree. Deleting a batch never makes another row
//! of any table eligible or ineligible (see each set), so a dry-run count is
//! exactly what sweeps down to zero remove.
//!
//! Protected sets:
//!
//! * **`work_journal`** (`work.retention.journal_days`, by `at`):
//!   - every row of an OPEN conversation (`conversations.ended_at IS NULL`);
//!   - every row of a conversation of a session behind a LIVE link
//!     (`ended_at IS NULL`, not `rejected`);
//!   - every row of a conversation a CONFIRMED link (live or ended)
//!     reaches — its snapshot ids, or its live session's conversations —
//!     while that link's work is not done: a bare `ref_key` link (no item,
//!     so no status), an item not `done`, or an item any live link names
//!     (by id or key). So the latest agent note of a live item, and a live
//!     session's primary work, keep their journal regardless of age;
//!   - an undelivered `handover`, and any `handover` addressed to a live
//!     participant.
//! * **`work_items`** (`work.retention.tracker_items_days`, by the newest of
//!   `created_at` / `updated_at` / `status_changed_at` / `fetched_at` /
//!   `reopened_at`): only tracker items (`tracker_id` set) in `done`; kept
//!   while any link (any state, live or ended — history and final
//!   rejections) names the item, or a link's `ref_key` equals its key or an
//!   alias, and kept as the ancestor of any kept item (recursive: a parent
//!   goes only with all its descendants). Local items are never swept.
//! * **`session_events`** (`work.retention.timeline_work_events_days`, by
//!   `at`): only [`WORK_EVENT_KINDS`]; the newest event of each kind per
//!   session is kept by BOTH orders its readers use — `at DESC, id DESC`
//!   (`newest_session_event_of`, a pending handover) and `MAX(id)` (the
//!   M11.3 keep in `tidy_sessions`) — so a clock step cannot drop either.
//!
//! There is no pinned-note concept in the schema; a note is kept by the
//! journal rules above. `0` days keeps a table forever.

use super::Store;
use crate::ipc_error::IpcError;

/// Timeline kinds the work graph writes: agent handover (M9.3), the
/// start-prompt handover of a resume (M2), the classification nudge (M4.6)
/// and tidy (M7). Detection writes no timeline event.
pub const WORK_EVENT_KINDS: &[&str] = &[
    "handover_requested",
    "handover_written",
    "handover_missing",
    "handover_send_failed",
    "handover_waiting",
    "handover_started",
    "work_classify_nudge",
    "gc_tidied",
    // A per-session keep (M11.3): its detail is the second it holds until.
    "tidy_kept",
];

/// One retention-swept table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionTable {
    Journal,
    TrackerItems,
    WorkEvents,
}

impl RetentionTable {
    pub const ALL: [RetentionTable; 3] = [
        RetentionTable::Journal,
        RetentionTable::TrackerItems,
        RetentionTable::WorkEvents,
    ];

    /// The SQL table.
    pub fn table(self) -> &'static str {
        match self {
            RetentionTable::Journal => "work_journal",
            RetentionTable::TrackerItems => "work_items",
            RetentionTable::WorkEvents => "session_events",
        }
    }

    /// `WITH … eligible(id)`; `?1` is the cutoff (unix seconds).
    fn cte(self) -> String {
        match self {
            RetentionTable::Journal => JOURNAL_CTE.to_string(),
            RetentionTable::TrackerItems => ITEMS_CTE.to_string(),
            RetentionTable::WorkEvents => events_cte(),
        }
    }

    /// Every row the table holds that this sweep is about.
    fn scope_sql(self) -> String {
        match self {
            RetentionTable::Journal => "SELECT COUNT(*) FROM work_journal".into(),
            RetentionTable::TrackerItems => {
                "SELECT COUNT(*) FROM work_items WHERE tracker_id IS NOT NULL".into()
            }
            RetentionTable::WorkEvents => format!(
                "SELECT COUNT(*) FROM session_events WHERE kind IN ({})",
                kinds_sql()
            ),
        }
    }
}

const JOURNAL_CTE: &str = "\
    WITH live AS ( \
      SELECT item_id, ref_key, participant_id FROM work_links \
      WHERE ended_at IS NULL AND state != 'rejected'), \
    kept_links AS ( \
      SELECT l.participant_id, l.ended_at, l.snap_claude_ids FROM work_links l \
      LEFT JOIN work_items i ON i.id = l.item_id \
      WHERE l.state = 'confirmed' AND ( \
        l.item_id IS NULL OR i.id IS NULL OR i.status_category != 'done' \
        OR l.item_id IN (SELECT item_id FROM live WHERE item_id IS NOT NULL) \
        OR i.key IN (SELECT ref_key FROM live WHERE ref_key IS NOT NULL))), \
    kept_conv(cid) AS ( \
      SELECT claude_session_id FROM conversations WHERE ended_at IS NULL \
      UNION SELECT c.claude_session_id FROM live l \
        JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
        JOIN conversations c ON c.session_id = p.session_id \
      UNION SELECT j.value FROM kept_links l, json_each(l.snap_claude_ids) j \
        WHERE l.snap_claude_ids IS NOT NULL \
      UNION SELECT c.claude_session_id FROM kept_links l \
        JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
        JOIN conversations c ON c.session_id = p.session_id), \
    eligible(id) AS ( \
      SELECT w.id FROM work_journal w WHERE w.at < ?1 \
        AND (w.kind != 'handover' OR (w.delivered_at IS NOT NULL \
             AND (w.participant_id IS NULL OR w.participant_id NOT IN \
                  (SELECT id FROM participants WHERE retired_at IS NULL)))) \
        AND (w.claude_session_id IS NULL OR w.claude_session_id NOT IN \
             (SELECT cid FROM kept_conv WHERE cid IS NOT NULL)))";

const ITEMS_CTE: &str = "\
    WITH RECURSIVE refs(k) AS ( \
      SELECT ref_key FROM work_links WHERE ref_key IS NOT NULL), \
    base(id) AS ( \
      SELECT i.id FROM work_items i \
      WHERE i.tracker_id IS NOT NULL AND i.status_category = 'done' \
        AND MAX(i.created_at, i.updated_at, COALESCE(i.status_changed_at, 0), \
                COALESCE(i.fetched_at, 0), COALESCE(i.reopened_at, 0)) < ?1 \
        AND i.id NOT IN (SELECT item_id FROM work_links WHERE item_id IS NOT NULL) \
        AND (i.key IS NULL OR i.key NOT IN (SELECT k FROM refs)) \
        AND NOT EXISTS (SELECT 1 FROM json_each(COALESCE(i.aliases, '[]')) a \
                        WHERE a.value IN (SELECT k FROM refs))), \
    kept(id) AS ( \
      SELECT id FROM work_items WHERE id NOT IN (SELECT id FROM base) \
      UNION SELECT w.parent_id FROM work_items w JOIN kept ON w.id = kept.id \
        WHERE w.parent_id IS NOT NULL), \
    eligible(id) AS ( \
      SELECT id FROM base WHERE id NOT IN (SELECT id FROM kept WHERE id IS NOT NULL))";

fn kinds_sql() -> String {
    WORK_EVENT_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn events_cte() -> String {
    let kinds = kinds_sql();
    format!(
        "WITH newest(id) AS ( \
           SELECT id FROM (SELECT id, ROW_NUMBER() OVER ( \
               PARTITION BY session_id, kind ORDER BY at DESC, id DESC) AS rn \
             FROM session_events WHERE kind IN ({kinds})) WHERE rn = 1 \
           UNION SELECT MAX(id) FROM session_events WHERE kind IN ({kinds}) \
             GROUP BY session_id, kind), \
         eligible(id) AS ( \
           SELECT e.id FROM session_events e \
           WHERE e.kind IN ({kinds}) AND e.at < ?1 \
             AND e.id NOT IN (SELECT id FROM newest))"
    )
}

/// The cutoff for a `days` window at `now`; `None` keeps forever.
pub fn retention_cutoff(now: i64, days: i64) -> Option<i64> {
    (days > 0).then(|| now - days * 86_400)
}

impl Store {
    /// Rows of `t` the retention sweep is about (the status count).
    pub fn retention_rows(&self, t: RetentionTable) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(&t.scope_sql(), [], |r| r.get(0))?)
    }

    /// How many rows of `t` a sweep would delete now with a `days` window
    /// (`0` = forever: none).
    pub fn retention_eligible(
        &self,
        t: RetentionTable,
        now: i64,
        days: i64,
    ) -> Result<i64, IpcError> {
        let Some(cutoff) = retention_cutoff(now, days) else {
            return Ok(0);
        };
        Ok(self.conn.query_row(
            &format!("{} SELECT COUNT(*) FROM eligible", t.cte()),
            rusqlite::params![cutoff],
            |r| r.get(0),
        )?)
    }

    /// Delete at most `limit` eligible rows of `t`, oldest ids first. One
    /// statement: the caller holds the lock for one batch only.
    pub fn retention_delete_batch(
        &self,
        t: RetentionTable,
        now: i64,
        days: i64,
        limit: usize,
    ) -> Result<usize, IpcError> {
        let Some(cutoff) = retention_cutoff(now, days) else {
            return Ok(0);
        };
        if limit == 0 {
            return Ok(0);
        }
        Ok(self.conn.execute(
            &format!(
                "{} DELETE FROM {} WHERE id IN (SELECT id FROM eligible ORDER BY id LIMIT ?2)",
                t.cte(),
                t.table()
            ),
            rusqlite::params![cutoff, limit as i64],
        )?)
    }
}

#[cfg(test)]
mod tests;
