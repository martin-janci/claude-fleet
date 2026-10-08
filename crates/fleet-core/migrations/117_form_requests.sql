-- Chat forms (docs/superpowers/specs/2026-10-07-chat-forms-design.md): a
-- form an agent asked a person to fill in its session's chat.
-- state  'pending' | 'answered' | 'declined' | 'cancelled' | 'expired'
-- answers  JSON {"answers": {...}, "secrets": {field: path}}: never a secret's value
-- secrets_on_host  1 while a secret directory for it may exist on the host
CREATE TABLE IF NOT EXISTS form_requests (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  form_id         TEXT    NOT NULL UNIQUE,
  session_id      INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
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

INSERT OR IGNORE INTO schema_version (version) VALUES (117);
