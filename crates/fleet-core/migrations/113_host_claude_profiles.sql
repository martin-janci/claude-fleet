-- 113: `hosts.claude_profiles`, the host's Claude login profiles
-- (`~/.claude-profiles/<name>`, docs/accounts.md) as a JSON array of
-- `{name, account_uuid, email}`, read by the reconcile probe. A session
-- under a profile is attributed to that profile's account, and the New
-- session dialog offers these names. NULL = never read.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN claude_profiles TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (113);
