ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'work';
ALTER TABLE sessions ADD COLUMN reviews_session_id INTEGER REFERENCES sessions(id);
INSERT OR IGNORE INTO schema_version (version) VALUES (5);
