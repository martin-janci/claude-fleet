-- Work graph M13.4e (decision D3 / D29): the write-back outbox.
--
-- One row per write fleet owes a tracker. Today the only op is
-- `pr_remote_link`: the pull request of a session a person linked
-- (`manual` / `started`) to an item, added to that item as a remote link.
-- The unique index makes a repeated trigger a no-op; the sync pass drains
-- due rows with backoff and marks them `done` or, after its last attempt,
-- `failed`. `session_org_id` is the session's org when the write was queued,
-- re-checked against the tracker's org before anything is sent.
CREATE TABLE IF NOT EXISTS tracker_writes (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  tracker_id        INTEGER NOT NULL REFERENCES trackers(id) ON DELETE CASCADE,
  item_key          TEXT    NOT NULL,
  op                TEXT    NOT NULL,
  url               TEXT    NOT NULL,
  title             TEXT    NOT NULL,
  link_id           INTEGER,
  claude_session_id TEXT,
  session_org_id    INTEGER,
  state             TEXT    NOT NULL DEFAULT 'pending',
  attempts          INTEGER NOT NULL DEFAULT 0,
  last_error        TEXT,
  next_at           INTEGER NOT NULL,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL,
  done_at           INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_tracker_writes_op
  ON tracker_writes(tracker_id, op, item_key, url);
CREATE INDEX IF NOT EXISTS idx_tracker_writes_due
  ON tracker_writes(tracker_id, next_at) WHERE state = 'pending';
INSERT OR IGNORE INTO schema_version (version) VALUES (61);
