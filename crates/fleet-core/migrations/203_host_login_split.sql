-- Whether a macOS host keeps its default Claude login twice: in the login
-- Keychain (where Claude Code stores it when it can reach the Keychain: the
-- desktop app, a local terminal) and in `~/.claude/.credentials.json`
-- (where it falls back to from a session started over SSH, whose Keychain
-- is locked). The two copies refresh independently, and each refresh
-- revokes the other's refresh token, so one side keeps reporting "Login
-- expired · Please run /login". The health probe asks whether both exist,
-- never what they hold. NULL = not macOS, or the host could not tell.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN login_split INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (203);
