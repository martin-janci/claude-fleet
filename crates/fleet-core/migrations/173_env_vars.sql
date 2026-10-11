-- Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md).
-- Exactly one of value / credential_ref. scope_key: '' (global), host alias,
-- 'owner/repo' (project) or the session id as text (session).
CREATE TABLE IF NOT EXISTS env_vars (
  id             INTEGER PRIMARY KEY,
  scope          TEXT NOT NULL CHECK (scope IN ('global','host','project','session')),
  scope_key      TEXT NOT NULL,
  name           TEXT NOT NULL,
  value          TEXT,
  credential_ref TEXT,
  secret         INTEGER NOT NULL DEFAULT 0,
  updated_at     INTEGER NOT NULL,
  UNIQUE (scope, scope_key, name),
  CHECK ((value IS NULL) <> (credential_ref IS NULL)),
  CHECK (scope <> 'global' OR scope_key = '')
);

CREATE TRIGGER IF NOT EXISTS trg_env_vars_on_session_delete
AFTER DELETE ON sessions
BEGIN
  DELETE FROM env_vars WHERE scope = 'session' AND scope_key = CAST(old.id AS TEXT);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (173);
