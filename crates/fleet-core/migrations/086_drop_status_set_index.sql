-- 084 created `idx_work_items_status_set` (a partial index on
-- `work_items.status_set_by`), but no query filters or joins on that column:
-- `effective_status_sql!` reads it per row and the derived stamp updates by
-- id. An unused index only costs writes, so it goes.
DROP INDEX IF EXISTS idx_work_items_status_set;

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
