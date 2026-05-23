-- Inter-session messages (peer-to-peer): a persisted inbox the recipient pulls
-- when ready, optionally accompanied by a real-time pane delivery from the
-- sender side. Rows are inserted on send and stamped with `read_at` when the
-- recipient reads them via the `inbox` tool. `kind` is open-ended (`message`,
-- `task`, `reply`, …) — receivers can filter on it. Reads are newest-first
-- via the (to_session_id, read_at, sent_at DESC) index.
CREATE TABLE session_messages (
  id INTEGER PRIMARY KEY,
  from_session_id INTEGER NOT NULL,
  to_session_id INTEGER NOT NULL,
  body TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'message',
  sent_at INTEGER NOT NULL,
  read_at INTEGER
);
CREATE INDEX idx_session_messages_inbox
  ON session_messages(to_session_id, read_at, sent_at DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (15);
