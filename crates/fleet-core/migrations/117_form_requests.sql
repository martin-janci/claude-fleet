-- Chat forms (docs/superpowers/specs/2026-10-07-chat-forms-design.md): a
-- form an agent asked a person to fill in its session's chat.
-- state  'pending' | 'answered' | 'declined' | 'cancelled' | 'expired'
-- answers  JSON {"answers": {...}, "secrets": {field: path}}: never a secret's value
-- secrets_on_host  1 while a secret directory for it may exist on the host
-- session_id  has NO foreign key: a hard-deleted session (delete_session, the
--   reconcile reap) must not take a form with secrets on the host with it, or
--   the tick sweep could never remove the directory. The trigger below keeps
--   such a form under the NEGATED session id (matches no session, and a
--   recycled sessions.id never inherits it); the sweep finds it by the missing
--   session, and the purge removes the row once swept and old.
CREATE TABLE IF NOT EXISTS form_requests (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  form_id         TEXT    NOT NULL UNIQUE,
  session_id      INTEGER NOT NULL,
  host_alias      TEXT    NOT NULL,
  spec            TEXT    NOT NULL,
  why             TEXT,
  state           TEXT    NOT NULL DEFAULT 'pending'
                  CHECK (state IN ('pending', 'answered', 'declined', 'cancelled', 'expired')),
  answers         TEXT,
  note            TEXT,
  answered_by     TEXT,
  secrets_on_host INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  decided_at      INTEGER
);
-- One pending form per session, and the cheap lookup the session row reads.
CREATE UNIQUE INDEX IF NOT EXISTS idx_form_requests_one_pending
  ON form_requests(session_id) WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS idx_form_requests_state
  ON form_requests(state, decided_at);

-- A deleted session: a form with secrets on the host is orphaned (pending ->
-- cancelled, answers and why cleared), every other form goes with the session.
CREATE TRIGGER IF NOT EXISTS form_requests_session_deleted AFTER DELETE ON sessions BEGIN
  UPDATE form_requests SET session_id = -OLD.id, answers = NULL, why = NULL,
         state = CASE WHEN state = 'pending' THEN 'cancelled' ELSE state END,
         decided_at = COALESCE(decided_at, CAST(strftime('%s','now') AS INTEGER))
   WHERE session_id = OLD.id AND secrets_on_host = 1;
  DELETE FROM form_requests WHERE session_id = OLD.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (117);
