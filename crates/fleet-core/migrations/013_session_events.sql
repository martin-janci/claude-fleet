-- Append-only per-session event timeline (Improvement C). Rows are inserted at
-- status transitions, prompt sends, stuck detection, and kill/recreate. Reads
-- are newest-first via the (session_id, at DESC) index. `kind` is one of:
-- status_change | prompt_sent | stuck | killed | recreated.
CREATE TABLE session_events (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,
  detail TEXT
);
CREATE INDEX idx_session_events_session ON session_events(session_id, at DESC);
INSERT OR IGNORE INTO schema_version (version) VALUES (13);
