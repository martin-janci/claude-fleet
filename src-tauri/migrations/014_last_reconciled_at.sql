-- Records the wall-clock time of the most recent reconcile pass that touched
-- this session row. Set on every reconcile upsert (background tick or pull).
-- Lets the frontend gray out stale rows when the proactive tick falls behind.
ALTER TABLE sessions ADD COLUMN last_reconciled_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (14);
