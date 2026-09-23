//! Remembered read cursors (migration 044). See the migration for why a
//! cursor is keyed by the reader's own session id rather than the caller.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

/// What a reader last saw of one resource through one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRow {
    pub watermark: Option<i64>,
    pub generation: Option<i64>,
    pub content_hash: Option<String>,
    /// `session_transcript` only: where in the transcript FILE the last
    /// read stopped, as a small JSON string (see the migration's comment
    /// on why `watermark` alone cannot serve this role). `None` for every
    /// other tool, and for a transcript cursor that never completed a
    /// `fresh_for` read.
    pub anchor: Option<String>,
}

impl Store {
    pub fn get_read_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
    ) -> Result<Option<CursorRow>, IpcError> {
        self.conn
            .query_row(
                "SELECT watermark, generation, content_hash, anchor FROM read_cursors \
                 WHERE reader_session_id = ?1 AND tool = ?2 AND resource_key = ?3",
                rusqlite::params![reader, tool, resource_key],
                |r| {
                    Ok(CursorRow {
                        watermark: r.get(0)?,
                        generation: r.get(1)?,
                        content_hash: r.get(2)?,
                        anchor: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Upsert a stream cursor. Clears any hash, so a row never carries both.
    /// `anchor` is `session_transcript`'s positional cursor (see
    /// [`CursorRow::anchor`]); every other stream tool passes `None`.
    #[allow(clippy::too_many_arguments)] // every field of one upserted row
    pub fn put_stream_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
        target: Option<i64>,
        watermark: i64,
        generation: Option<i64>,
        anchor: Option<&str>,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO read_cursors \
               (reader_session_id, tool, resource_key, target_session_id, \
                watermark, generation, anchor, content_hash, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8) \
             ON CONFLICT(reader_session_id, tool, resource_key) DO UPDATE SET \
               target_session_id = excluded.target_session_id, \
               watermark = excluded.watermark, generation = excluded.generation, \
               anchor = excluded.anchor, \
               content_hash = NULL, updated_at = excluded.updated_at",
            rusqlite::params![
                reader,
                tool,
                resource_key,
                target,
                watermark,
                generation,
                anchor,
                now_unix()
            ],
        )?;
        Ok(())
    }

    /// Upsert a snapshot cursor. Clears any watermark/generation/anchor.
    pub fn put_snapshot_cursor(
        &self,
        reader: i64,
        tool: &str,
        resource_key: &str,
        target: Option<i64>,
        content_hash: &str,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO read_cursors \
               (reader_session_id, tool, resource_key, target_session_id, \
                watermark, generation, anchor, content_hash, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6) \
             ON CONFLICT(reader_session_id, tool, resource_key) DO UPDATE SET \
               target_session_id = excluded.target_session_id, \
               watermark = NULL, generation = NULL, anchor = NULL, \
               content_hash = excluded.content_hash, updated_at = excluded.updated_at",
            rusqlite::params![reader, tool, resource_key, target, content_hash, now_unix()],
        )?;
        Ok(())
    }

    /// Delete cursors whose reader or (non-NULL) target session no longer
    /// exists. Immediate, no retention window: unlike undelivered mail, a
    /// cursor has no one to inform once its reader or target is gone.
    pub fn sweep_orphan_read_cursors(&self) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM read_cursors WHERE \
               reader_session_id NOT IN (SELECT id FROM sessions) \
               OR (target_session_id IS NOT NULL \
                   AND target_session_id NOT IN (SELECT id FROM sessions))",
            [],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    #[test]
    fn a_missing_cursor_is_none() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        assert!(s
            .get_read_cursor(r, "session_history", "7")
            .unwrap()
            .is_none());
    }

    #[test]
    fn put_is_an_upsert_per_reader_tool_and_resource() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        s.put_stream_cursor(r, "session_history", "7", Some(7), 10, None, None)
            .unwrap();
        s.put_stream_cursor(r, "session_history", "7", Some(7), 25, None, None)
            .unwrap();
        let c = s
            .get_read_cursor(r, "session_history", "7")
            .unwrap()
            .unwrap();
        assert_eq!(c.watermark, Some(25), "the second put replaces the first");
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM read_cursors", [], |x| x.get(0))
            .unwrap();
        assert_eq!(n, 1, "one row per (reader, tool, resource), never two");
    }

    /// THE regression test for the defect this cycle's design exists to
    /// avoid: a caller label is `host:<alias>`, so every session on a host is
    /// one caller, and a caller-keyed cursor would let them consume each
    /// other's deltas. Two readers of one resource keep independent cursors.
    #[test]
    fn two_readers_of_one_resource_keep_independent_cursors() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "reader-a");
        let b = seed(&s, "reader-b");
        s.put_stream_cursor(a, "session_transcript", "9", Some(9), 3, Some(1), None)
            .unwrap();
        s.put_stream_cursor(b, "session_transcript", "9", Some(9), 8, Some(1), None)
            .unwrap();
        assert_eq!(
            s.get_read_cursor(a, "session_transcript", "9")
                .unwrap()
                .unwrap()
                .watermark,
            Some(3)
        );
        assert_eq!(
            s.get_read_cursor(b, "session_transcript", "9")
                .unwrap()
                .unwrap()
                .watermark,
            Some(8)
        );
    }

    /// `session_transcript`'s positional cursor: stored, round-tripped, and
    /// replaced by a later put — same upsert semantics as `watermark`.
    #[test]
    fn a_stream_cursors_anchor_round_trips_and_is_replaced_by_a_later_put() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        let a1 = r#"{"at":"2026-01-01T00:00:00Z","ended_at":"2026-01-01T00:00:01Z"}"#;
        s.put_stream_cursor(r, "session_transcript", "9", Some(9), 3, None, Some(a1))
            .unwrap();
        assert_eq!(
            s.get_read_cursor(r, "session_transcript", "9")
                .unwrap()
                .unwrap()
                .anchor
                .as_deref(),
            Some(a1)
        );
        let a2 = r#"{"at":"2026-01-01T00:00:05Z","ended_at":null}"#;
        s.put_stream_cursor(r, "session_transcript", "9", Some(9), 4, None, Some(a2))
            .unwrap();
        assert_eq!(
            s.get_read_cursor(r, "session_transcript", "9")
                .unwrap()
                .unwrap()
                .anchor
                .as_deref(),
            Some(a2),
            "a later put replaces the anchor, same as watermark"
        );
    }

    #[test]
    fn a_snapshot_cursor_stores_a_hash_and_no_watermark() {
        let s = Store::open_in_memory().unwrap();
        let r = seed(&s, "reader");
        s.put_snapshot_cursor(r, "list_sessions", "all", None, "abc")
            .unwrap();
        let c = s
            .get_read_cursor(r, "list_sessions", "all")
            .unwrap()
            .unwrap();
        assert_eq!(c.content_hash.as_deref(), Some("abc"));
        assert_eq!(c.watermark, None);
        assert_eq!(c.anchor, None);
    }

    #[test]
    fn the_sweep_removes_cursors_whose_reader_or_target_is_gone() {
        let s = Store::open_in_memory().unwrap();
        let reader = seed(&s, "reader");
        let target = seed(&s, "target");
        let keep = seed(&s, "keep");
        s.put_stream_cursor(
            reader,
            "session_history",
            &target.to_string(),
            Some(target),
            1,
            None,
            None,
        )
        .unwrap();
        s.put_stream_cursor(
            keep,
            "session_history",
            &keep.to_string(),
            Some(keep),
            1,
            None,
            None,
        )
        .unwrap();
        s.put_snapshot_cursor(keep, "list_sessions", "all", None, "h")
            .unwrap();
        s.delete_session(target).unwrap();
        assert_eq!(
            s.sweep_orphan_read_cursors().unwrap(),
            1,
            "only the cursor ON the gone target"
        );
        s.delete_session(reader).unwrap();
        assert_eq!(
            s.sweep_orphan_read_cursors().unwrap(),
            0,
            "its only cursor was already swept"
        );
        assert!(s
            .get_read_cursor(keep, "session_history", &keep.to_string())
            .unwrap()
            .is_some());
        assert!(
            s.get_read_cursor(keep, "list_sessions", "all")
                .unwrap()
                .is_some(),
            "a NULL target is not an orphan — list_sessions has no target session"
        );
        s.delete_session(keep).unwrap();
        assert_eq!(
            s.sweep_orphan_read_cursors().unwrap(),
            2,
            "a gone READER takes all its cursors"
        );
    }
}
