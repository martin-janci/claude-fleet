-- Per-host control-API bearer tokens (Wave 1 Track B, SEC-1). Provisioning a
-- host mints (or reuses) that host's token and writes it into the remote
-- ~/.claude.json instead of the single master token, so a token lifted from
-- one machine identifies — and is scoped to — that machine. `mode` is
-- 'full' (every tool) or 'readonly' (mutating tools rejected with
-- E_FORBIDDEN). The master token in `settings` stays valid for the desktop
-- and local clients.
CREATE TABLE host_tokens (
  host_alias  TEXT PRIMARY KEY,
  token       TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  mode        TEXT NOT NULL DEFAULT 'full'
);
CREATE UNIQUE INDEX idx_host_tokens_token ON host_tokens(token);

INSERT OR IGNORE INTO schema_version (version) VALUES (18);
