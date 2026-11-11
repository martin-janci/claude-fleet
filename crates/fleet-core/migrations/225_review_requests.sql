-- Review requests (HumanLayer plan v2, R11 lean): "Ask for review…" shares
-- a session with a colleague and puts "X asks you to review" in their Inbox.
-- The share itself is an ordinary grant (session_grants); this row is the
-- ask, which the colleague marks done.
--
-- review_requests
--   session_id      the session whose changes are to be reviewed
--   from_person_id  who asked (the session's owner)
--   to_person_id    who is asked
--   level           the grant the ask came with: drive (may send notes to
--                   the agent) or watch (reads and drafts only: a viewer)
--   note            what the asker wrote, if anything
--   created_at      when it was asked
--   done_at         when the colleague marked it done (NULL: open)
-- An open ask per session and colleague at most: asking again refreshes it.
CREATE TABLE IF NOT EXISTS review_requests (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id      INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  from_person_id  INTEGER NOT NULL,
  to_person_id    INTEGER NOT NULL,
  level           TEXT    NOT NULL,
  note            TEXT,
  created_at      INTEGER NOT NULL,
  done_at         INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS review_requests_open
  ON review_requests (session_id, to_person_id) WHERE done_at IS NULL;
CREATE INDEX IF NOT EXISTS review_requests_to
  ON review_requests (to_person_id, done_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (225);
