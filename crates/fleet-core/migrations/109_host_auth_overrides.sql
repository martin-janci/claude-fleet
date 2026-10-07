-- Which credential variables outrank the host's `/login` (multi-account
-- groundwork). Claude Code picks a cloud provider, `ANTHROPIC_AUTH_TOKEN`,
-- `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` over the subscription
-- login, so a host whose shell or tmux server exports one bills that
-- credential while fleet shows the logged-in account. The health probe
-- reports the NAMES it finds (never a value), as a JSON string array.
-- NULL = the host could not tell; '[]' = none set.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN auth_overrides TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (109);
