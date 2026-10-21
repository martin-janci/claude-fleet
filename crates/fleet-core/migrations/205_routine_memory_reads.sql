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
-- query's own predicates, so it holds the unread finished runs only. A
-- run of a routine that keeps no notebook has nothing to read: it is
-- stamped read as it finishes (`Store::finish_routine_run`,
-- `insert_routine_run`), and the ones already finished are stamped here,
-- so the index does not grow with every such run's history.
ALTER TABLE routine_runs ADD COLUMN memory_retry_at INTEGER;

UPDATE routine_runs SET memory_read_at = finished_at
  WHERE memory_read_at IS NULL AND state IN ('done', 'failed')
    AND routine_id IN (SELECT id FROM routines WHERE memory IS NULL);

CREATE INDEX IF NOT EXISTS routine_runs_memory_unread
  ON routine_runs (finished_at, id)
  WHERE memory_read_at IS NULL AND state IN ('done', 'failed');

INSERT OR IGNORE INTO schema_version (version) VALUES (205);
