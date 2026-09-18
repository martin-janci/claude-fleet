-- claude_session_id is added by 009_session_claude_id.sql; this migration adds
-- the remaining Claude-agent intelligence columns.
ALTER TABLE sessions ADD COLUMN claude_status TEXT;
ALTER TABLE sessions ADD COLUMN effort_level TEXT;
ALTER TABLE sessions ADD COLUMN pr_url TEXT;
ALTER TABLE sessions ADD COLUMN current_activity TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (10);
