//! The full-text index (search phase 3, migration 163): `search_docs` and
//! its FTS5 `search_fts`. The migration's triggers keep the docs of work
//! items, sessions, conversations, pull requests and the journal in step
//! with their rows; transcript chunks are written here by the transcript
//! pass (`service/search_index.rs`), which is off by default.
//!
//! Reads answer raw matches; who may see each one is the service's
//! (`service/search.rs`), which fences every hit before it leaves the hub.

use super::Store;
use crate::ipc_error::IpcError;

/// One match, before any fence.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchDocHit {
    pub id: i64,
    pub kind: String,
    pub ref_id: String,
    pub session_id: Option<i64>,
    pub claude_session_id: Option<String>,
    pub item_id: Option<i64>,
    /// The title with its matches between [`MARK_OPEN`] and [`MARK_CLOSE`].
    pub title: String,
    /// A few words around the body's matches, marked the same way.
    pub snippet: String,
    pub at: i64,
    /// FTS5's bm25, lower is better.
    pub rank: f64,
}

/// What `highlight` / `snippet` put around a match: private-use code
/// points no source text holds, turned into ranges by the service.
pub const MARK_OPEN: char = '\u{E000}';
pub const MARK_CLOSE: char = '\u{E001}';

/// A transcript chunk the transcript pass indexes.
#[derive(Debug, Clone)]
pub struct TranscriptChunk<'a> {
    pub session_id: i64,
    pub claude_session_id: &'a str,
    /// The byte offset in the transcript the chunk starts at: with the
    /// claude id, the chunk's identity, so a re-read never doubles it.
    pub offset: u64,
    pub text: &'a str,
    pub at: i64,
}

impl Store {
    /// Up to `limit` docs matching `fts_query` (an FTS5 expression the
    /// service built), best first, of the `kinds` given (every kind when
    /// empty).
    pub fn search_docs_matching(
        &self,
        fts_query: &str,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<SearchDocHit>, IpcError> {
        let open = MARK_OPEN.to_string();
        let close = MARK_CLOSE.to_string();
        // The kinds are a fixed vocabulary the service checked; they are
        // bound, never spliced.
        let kinds_json = serde_json::to_string(kinds).unwrap_or_else(|_| "[]".into());
        let mut stmt = self.conn.prepare_cached(
            "SELECT d.id, d.kind, d.ref, d.session_id, d.claude_session_id, d.item_id, \
                    highlight(search_fts, 0, ?2, ?3), \
                    snippet(search_fts, 1, ?2, ?3, '…', 16), \
                    d.at, bm25(search_fts, 4.0, 1.0) AS rank \
               FROM search_fts JOIN search_docs d ON d.id = search_fts.rowid \
              WHERE search_fts MATCH ?1 \
                AND (json_array_length(?4) = 0 OR d.kind IN (SELECT value FROM json_each(?4))) \
              ORDER BY rank, d.at DESC \
              LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![fts_query, open, close, kinds_json, limit as i64],
            |r| {
                Ok(SearchDocHit {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    ref_id: r.get(2)?,
                    session_id: r.get(3)?,
                    claude_session_id: r.get(4)?,
                    item_id: r.get(5)?,
                    title: r.get(6)?,
                    snippet: r.get(7)?,
                    at: r.get(8)?,
                    rank: r.get(9)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Index one transcript chunk; the same chunk again replaces itself.
    pub fn upsert_transcript_chunk(&self, c: &TranscriptChunk<'_>) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO search_docs (kind, ref, session_id, claude_session_id, title, body, at) \
             VALUES ('transcript', ?1, ?2, ?3, '', ?4, ?5) \
             ON CONFLICT (kind, ref) DO UPDATE SET body = excluded.body, at = excluded.at",
            rusqlite::params![
                format!("{}:{}", c.claude_session_id, c.offset),
                c.session_id,
                c.claude_session_id,
                c.text,
                c.at,
            ],
        )?;
        Ok(())
    }

    /// Drop transcript chunks older than `before` (unix seconds); how many
    /// went.
    pub fn prune_transcript_chunks(&self, before: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM search_docs WHERE kind = 'transcript' AND at < ?1",
            [before],
        )?)
    }

    /// Drop every transcript chunk and every cursor: the setting was turned
    /// off, and what was copied in goes with it.
    pub fn clear_transcript_chunks(&self) -> Result<usize, IpcError> {
        self.conn
            .execute("DELETE FROM search_transcript_cursors", [])?;
        Ok(self
            .conn
            .execute("DELETE FROM search_docs WHERE kind = 'transcript'", [])?)
    }

    /// Drop one conversation's chunks (its transcript was rewritten).
    pub fn clear_transcript_chunks_of(&self, claude_session_id: &str) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM search_docs WHERE kind = 'transcript' AND claude_session_id = ?1",
            [claude_session_id],
        )?)
    }

    /// session id → (file name, bytes indexed) for every cursor.
    pub fn transcript_cursors(
        &self,
    ) -> Result<std::collections::HashMap<i64, (String, i64)>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT session_id, source, offset_bytes FROM search_transcript_cursors")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record how far the pass got in a session's transcript.
    pub fn set_transcript_cursor(
        &self,
        session_id: i64,
        source: &str,
        offset_bytes: i64,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO search_transcript_cursors (session_id, source, offset_bytes, updated_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (session_id) DO UPDATE SET \
               source = excluded.source, offset_bytes = excluded.offset_bytes, \
               updated_at = excluded.updated_at",
            rusqlite::params![session_id, source, offset_bytes, now],
        )?;
        Ok(())
    }

    /// How many docs of each kind the index holds (diagnostics).
    pub fn search_doc_counts(&self) -> Result<Vec<(String, i64)>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) FROM search_docs GROUP BY kind ORDER BY kind")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
