-- 068: usage_daily keyed by (day, host_alias, backfill). A first read of an
-- existing transcript books that transcript's whole history in one pass;
-- those rows are `backfill = 1` and reported apart (perf-logs §6a). Existing
-- rows become live (backfill = 0) rows. Guarded by `usage_daily_has_backfill`
-- in schema.rs, so a re-run cannot collapse backfill rows into live ones.
-- (The plan numbered this 062; 061-067 landed on main first. MIGRATIONS
-- must stay contiguous, so this takes the next free number.)
-- Rebuilt WITHOUT `ALTER TABLE … RENAME`: a rename re-parses every trigger in
-- the schema, and on a database whose `main` 034-036 columns are still
-- missing (the conversation-branch collision `repair_skipped_main_migrations`
-- fixes AFTER the pending migrations) 063's `sessions_row_version_bump`
-- names `NEW.lost_reason` and the rename fails. Copy out, drop, recreate,
-- copy back: no rename, same result.
CREATE TABLE IF NOT EXISTS usage_daily_rebuild (
    day INTEGER NOT NULL,
    host_alias TEXT NOT NULL,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, host_alias, backfill)
);
INSERT OR IGNORE INTO usage_daily_rebuild
    (day, host_alias, backfill, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros)
    SELECT day, host_alias, 0, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros
    FROM usage_daily;
DROP TABLE usage_daily;
CREATE TABLE usage_daily (
    day INTEGER NOT NULL,
    host_alias TEXT NOT NULL,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, host_alias, backfill)
);
INSERT INTO usage_daily
    (day, host_alias, backfill, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros)
    SELECT day, host_alias, backfill, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros
    FROM usage_daily_rebuild;
DROP TABLE usage_daily_rebuild;

INSERT OR IGNORE INTO schema_version (version) VALUES (71);
