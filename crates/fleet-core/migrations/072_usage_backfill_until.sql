-- 072: `sessions.usage_backfill_until`, the transcript size a usage cursor
-- saw when it started reading a file from byte 0 (a first read, another
-- file, or a rewrite). A transcript larger than one pass's chunk
-- (`MAX_CHUNK_BYTES`) is read over several passes; every read that starts
-- below this mark is still that file's history, so its earlier days are
-- booked as backfill like the first chunk's (perf-logs §6a). 0 = no history
-- pending. Per-pass bookkeeping, not a `SessionRow` field: the row-version
-- trigger does not watch it. One ADD COLUMN, guarded in schema.rs.
ALTER TABLE sessions ADD COLUMN usage_backfill_until INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (72);
