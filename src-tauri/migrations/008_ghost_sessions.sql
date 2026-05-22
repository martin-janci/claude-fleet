ALTER TABLE sessions ADD COLUMN lost_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (8);
