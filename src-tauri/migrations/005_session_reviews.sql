ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'work';
-- ON DELETE SET NULL: a review is itself a session; if its source session is
-- deleted (e.g. reconcile's delete_sessions_not_in drops a killed work session
-- while its review is still live), the review survives and its link just nulls.
-- Without this, the self-FK with foreign_keys=ON would make the source delete
-- fail and abort the whole reconcile for that host.
ALTER TABLE sessions ADD COLUMN reviews_session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (5);
