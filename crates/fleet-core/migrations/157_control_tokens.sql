-- Named Control API tokens (Orbit Fleet M15 step G2.8): a token the owner
-- creates for a script or another agent, with a scope, an expiry and an
-- optional host limit. Shown once at creation; the store keeps only its
-- SHA-256, as client_tokens does.
--   name          unique among live rows (a revoked row frees its name)
--   token_sha256  lowercase hex SHA-256 of the token; never the token
--   scope         read  | act (sessions, not fleet admin) | admin (as the
--                 master token)
--   hosts         JSON array of host aliases the token may reach; NULL =
--                 every host. Never set on an admin token.
--   expires_at    unix seconds after which the token is refused; NULL =
--                 never
--   last_used_at  the last request it authenticated, stamped at most once
--                 a minute
--   revoked_at    set by Revoke; a revoked row never resolves again
CREATE TABLE IF NOT EXISTS control_tokens (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL,
  token_sha256  TEXT NOT NULL UNIQUE,
  scope         TEXT NOT NULL CHECK (scope IN ('read', 'act', 'admin')),
  hosts         TEXT,
  expires_at    INTEGER,
  created_at    INTEGER NOT NULL,
  last_used_at  INTEGER,
  revoked_at    INTEGER,
  CHECK (scope <> 'admin' OR hosts IS NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS control_tokens_live_name
  ON control_tokens(name) WHERE revoked_at IS NULL;

-- The hub's token cache rebuilds when auth_epoch moves (migration 060): every
-- write that changes what a token resolves to bumps it, stamping
-- last_used_at does not.
CREATE TRIGGER IF NOT EXISTS auth_epoch_control_tokens_insert
  AFTER INSERT ON control_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_control_tokens_update
  AFTER UPDATE ON control_tokens
  WHEN OLD.id IS NOT NEW.id
    OR OLD.name IS NOT NEW.name
    OR OLD.token_sha256 IS NOT NEW.token_sha256
    OR OLD.scope IS NOT NEW.scope
    OR OLD.hosts IS NOT NEW.hosts
    OR OLD.expires_at IS NOT NEW.expires_at
    OR OLD.created_at IS NOT NEW.created_at
    OR OLD.revoked_at IS NOT NEW.revoked_at
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_control_tokens_delete
  AFTER DELETE ON control_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (157);
