-- Remembered read cursors (smart caching, cycle 2). A reader re-asking a
-- fetch tool with `fresh_for` gets only what is new. Keyed by the READER'S
-- OWN session id, never the caller: a caller label is `host:<alias>`, so
-- every Claude session on a host is the same caller, and a caller-keyed
-- cursor would let them consume each other's deltas by default.
--
-- A row uses `watermark` (+ `generation`) for an append-only stream, or
-- `content_hash` for a snapshot — never both.
--
-- `generation`: for session_transcript, the id of the latest
-- conversation-boundary event when the cursor was written. turn_seq keeps
-- counting across /clear while the transcript FILE changes, so without it a
-- /clear between two reads would silently skip the old conversation's tail.
--
-- `target_session_id`: the session a cursor is ABOUT (NULL for
-- list_sessions, which has none), so the GC can drop cursors whose target
-- is gone. No foreign keys, matching the rest of this schema: dangling ids
-- are a tolerated state and the sweep is what cleans them.
CREATE TABLE IF NOT EXISTS read_cursors (
  id INTEGER PRIMARY KEY,
  reader_session_id INTEGER NOT NULL,
  tool TEXT NOT NULL,
  resource_key TEXT NOT NULL,
  target_session_id INTEGER,
  watermark INTEGER,
  generation INTEGER,
  content_hash TEXT,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_read_cursors_key
  ON read_cursors(reader_session_id, tool, resource_key);
CREATE INDEX IF NOT EXISTS idx_read_cursors_target
  ON read_cursors(target_session_id) WHERE target_session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (44);
