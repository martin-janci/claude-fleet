-- Routine memory's reads (migration 204, `service::routines::memory`).
--
-- routine_runs
--   memory_retry_at  a run whose transcript could not be read is not read
--                    again before this (unix seconds); NULL = any time. A
--                    host that stays unreachable then no longer holds the
--                    oldest-first window of every pass, nor takes an SSH
--                    attempt per run and pass.
--
-- `routine_runs_memory_unread` covers the scheduler's per-tick probe
-- (`Store::routine_runs_memory_unread`): no index fit its filter, so each
-- tick scanned the runs' whole history under the store lock (the same
-- problem migration 146 fixed for the unjudged runs). Partial on the
-- query's own predicates, so it holds the unread finished runs only.
ALTER TABLE routine_runs ADD COLUMN memory_retry_at INTEGER;

CREATE INDEX IF NOT EXISTS routine_runs_memory_unread
  ON routine_runs (finished_at, id)
  WHERE memory_read_at IS NULL AND state IN ('done', 'failed');

INSERT OR IGNORE INTO schema_version (version) VALUES (205);
