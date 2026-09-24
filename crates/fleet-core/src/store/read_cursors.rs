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
    /// exists. No retention window: unlike undelivered mail, a cursor has no
    /// one to inform once its reader or target is gone. A BACKSTOP: every
    /// session delete already drops that session's cursors in the same
    /// statement (migration 044's `trg_read_cursors_on_session_delete`), so
    /// this finds only rows that name an id no session ever had.
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
        // The store treats the anchor as opaque text; the shape shown is
        // `service::transcript::TranscriptAnchor`'s — `at` plus a hex
        // SHA-256 `fingerprint` of the turn's rendered text.
        let a1 = r#"{"at":"2026-01-01T00:00:00Z","fingerprint":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"}"#;
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
        let a2 = r#"{"at":"2026-01-01T00:00:05Z","fingerprint":"60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752"}"#;
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

    fn cursor_count(s: &Store) -> i64 {
        s.conn
            .query_row("SELECT COUNT(*) FROM read_cursors", [], |x| x.get(0))
            .unwrap()
    }

    /// Ruling 16 (reader-id reuse): `sessions.id` has no AUTOINCREMENT, so
    /// deleting the highest-id session hands that id to the NEXT session
    /// created. Its cursors must die with the row, at once — not whenever the
    /// GC sweep next runs — or the new session inherits a dead session's
    /// "already read" and its first read answers `unchanged`.
    #[test]
    fn a_new_session_reusing_a_deleted_readers_id_inherits_no_cursor() {
        let s = Store::open_in_memory().unwrap();
        let _other = seed(&s, "other");
        let reader = seed(&s, "reviewer");
        s.put_snapshot_cursor(reader, "list_sessions", "all", None, "h")
            .unwrap();
        s.put_stream_cursor(reader, "session_history", "1", Some(1), 9, None, None)
            .unwrap();
        s.delete_session(reader).unwrap();
        let reborn = seed(&s, "reviewer-2");
        assert_eq!(reborn, reader, "SQLite reuses the highest deleted id");
        assert!(
            s.get_read_cursor(reborn, "list_sessions", "all")
                .unwrap()
                .is_none(),
            "a reused reader id must not inherit the dead reader's snapshot cursor"
        );
        assert!(
            s.get_read_cursor(reborn, "session_history", "1")
                .unwrap()
                .is_none(),
            "nor its stream cursor"
        );
    }

    /// Target-id reuse: a cursor ABOUT a deleted session goes with it, at
    /// once, without a sweep — its events are deleted too, so event ids (and
    /// the target id itself) can be reused under the old watermark.
    #[test]
    fn deleting_a_target_session_drops_cursors_about_it_without_a_sweep() {
        let s = Store::open_in_memory().unwrap();
        let reader = seed(&s, "reader");
        let target = seed(&s, "target");
        s.put_stream_cursor(
            reader,
            "session_history",
            &target.to_string(),
            Some(target),
            4,
            None,
            None,
        )
        .unwrap();
        s.put_snapshot_cursor(reader, "list_sessions", "all", None, "h")
            .unwrap();
        s.delete_session(target).unwrap();
        assert!(
            s.get_read_cursor(reader, "session_history", &target.to_string())
                .unwrap()
                .is_none(),
            "the cursor on the deleted target must be gone immediately"
        );
        assert!(
            s.get_read_cursor(reader, "list_sessions", "all")
                .unwrap()
                .is_some(),
            "the live reader's other cursors are untouched"
        );
    }

    /// Every session delete path cleans up at once (the trigger), so the
    /// sweep is now a BACKSTOP: it finds nothing after an ordinary delete,
    /// and still removes rows that name an id with no session behind it —
    /// e.g. a cursor written with a dangling reader or target id.
    #[test]
    fn deletes_clean_up_eagerly_and_the_sweep_is_a_backstop_for_dangling_ids() {
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
        assert_eq!(cursor_count(&s), 3);

        s.delete_session(target).unwrap();
        assert_eq!(
            cursor_count(&s),
            2,
            "the cursor ON the gone target went with it"
        );
        s.delete_session(reader).unwrap();
        assert_eq!(cursor_count(&s), 2, "reader's only cursor was already gone");
        assert_eq!(
            s.sweep_orphan_read_cursors().unwrap(),
            0,
            "the trigger left the sweep nothing to find"
        );

        // Backstop: rows naming ids no session ever had (a dangling reader,
        // a dangling non-NULL target) — nothing deleted a session for these,
        // so only the sweep can find them.
        s.put_snapshot_cursor(9_001, "list_sessions", "all", None, "h")
            .unwrap();
        s.put_stream_cursor(keep, "session_history", "9002", Some(9_002), 1, None, None)
            .unwrap();
        assert_eq!(cursor_count(&s), 4);
        assert_eq!(
            s.sweep_orphan_read_cursors().unwrap(),
            2,
            "the sweep removes the dangling reader and the dangling target"
        );
        assert!(
            s.get_read_cursor(keep, "list_sessions", "all")
                .unwrap()
                .is_some(),
            "a NULL target is not an orphan — list_sessions has no target session"
        );

        s.delete_session(keep).unwrap();
        assert_eq!(cursor_count(&s), 0, "a gone READER takes all its cursors");
    }
}
