ALTER TABLE sessions ADD COLUMN claude_session_id TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (9);
