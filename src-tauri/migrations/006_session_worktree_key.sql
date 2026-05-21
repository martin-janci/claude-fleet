ALTER TABLE sessions ADD COLUMN worktree_key TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (6);
