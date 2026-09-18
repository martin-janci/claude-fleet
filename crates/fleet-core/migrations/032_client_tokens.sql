-- Client tokens: one row per paired client (a phone, a laptop browser).
-- Unlike host_tokens, only the SHA-256 of the token is stored: the plaintext
-- is shown once at pairing and never needs to be displayed again.
CREATE TABLE IF NOT EXISTS client_tokens (
  id            INTEGER PRIMARY KEY,
  name          TEXT    NOT NULL,
  token_sha256  TEXT    NOT NULL,
  mode          TEXT    NOT NULL DEFAULT 'full',
  created_at    INTEGER NOT NULL,
  last_seen_at  INTEGER,
  revoked_at    INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_client_tokens_hash ON client_tokens(token_sha256);
-- A name is unique among *live* rows; a revoked row keeps its name for the
-- audit trail and does not block re-pairing under the same name.
CREATE UNIQUE INDEX IF NOT EXISTS idx_client_tokens_live_name
  ON client_tokens(name) WHERE revoked_at IS NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (32);
