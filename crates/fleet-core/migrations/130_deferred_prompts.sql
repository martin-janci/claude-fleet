-- Deferred prompts (redesign step 5.10, "Send prompt"): a prompt for a
-- session that was busy when it was sent, typed in as a new turn once the
-- session is idle again (`service::sessions::deferred`).
--   body          the text exactly as it will be typed (an agent caller's
--                 untrusted marker included)
--   delivered_at  set when the prompt is claimed for typing; cleared again
--                 when that attempt fails and a retry is left
--   attempts      typing attempts that failed
--   failed_at     set once the attempts ran out; `error` says why
--   cancelled_at  a person took it back before it went out
-- A pending row has all three stamps NULL. Rows go with their session.
CREATE TABLE IF NOT EXISTS deferred_prompts (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id    INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  body          TEXT    NOT NULL,
  created_at    INTEGER NOT NULL,
  delivered_at  INTEGER,
  attempts      INTEGER NOT NULL DEFAULT 0,
  failed_at     INTEGER,
  error         TEXT,
  cancelled_at  INTEGER
);

CREATE INDEX IF NOT EXISTS deferred_prompts_pending
  ON deferred_prompts (session_id, id)
  WHERE delivered_at IS NULL AND failed_at IS NULL AND cancelled_at IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (130);
