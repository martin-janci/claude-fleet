-- Orbit Fleet redesign step 11.4, the Control API tokens table: when a
-- host's token was last used and last rotated. Both nullable: a token never
-- used since this migration reads "never", and one never rotated has no
-- rotation.
--   last_used_at  the last request this token authenticated, stamped at
--                 most once a minute (as client_tokens.last_seen_at is)
--   rotated_at    when a fresh token last replaced this host's; created_at
--                 now keeps the first mint
ALTER TABLE host_tokens ADD COLUMN last_used_at INTEGER;
ALTER TABLE host_tokens ADD COLUMN rotated_at INTEGER;

-- Migration 060 bumps auth_epoch on every update of host_tokens. Stamping
-- liveness must not rebuild the hub's token cache once a minute per host,
-- so the update trigger now lists the columns that change what a token
-- resolves to, as auth_epoch_client_tokens_update does: everything but
-- last_used_at.
DROP TRIGGER IF EXISTS auth_epoch_host_tokens_update;
CREATE TRIGGER IF NOT EXISTS auth_epoch_host_tokens_update
  AFTER UPDATE ON host_tokens
  WHEN OLD.host_alias IS NOT NEW.host_alias
    OR OLD.token IS NOT NEW.token
    OR OLD.created_at IS NOT NEW.created_at
    OR OLD.mode IS NOT NEW.mode
    OR OLD.rotated_at IS NOT NEW.rotated_at
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (126);
