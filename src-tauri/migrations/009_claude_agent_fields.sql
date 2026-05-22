ALTER TABLE sessions ADD COLUMN claude_session_id TEXT;
ALTER TABLE sessions ADD COLUMN claude_status TEXT;
ALTER TABLE sessions ADD COLUMN effort_level TEXT;
ALTER TABLE sessions ADD COLUMN pr_url TEXT;
ALTER TABLE sessions ADD COLUMN current_activity TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (9);
