-- Gap plan G7.4 (Automation forms board, "Delete keeps its runs"): a
-- routine's runs outlive it. `routine_runs.routine_id` (131) was NOT NULL
-- with ON DELETE CASCADE, so deleting a routine erased its history from
-- Runs. It becomes nullable with ON DELETE SET NULL, and `routine_name`
-- keeps what the run was called: `Store::delete_routine` writes the name
-- onto the runs before the routine goes, and Runs reads it for a run whose
-- routine is gone (`store::runs::ROUTINE_BRANCH`).
--
-- SQLite cannot change a column's constraints in place, so the table is
-- rebuilt as 142 does it: copy the rows aside, drop, create it again under
-- its own name, copy back (ids preserved: decision runs and Inbox items
-- name runs by id), and recreate the five indexes 131, 141 and 146 wrote.
-- `Store::migrate` runs this with foreign keys OFF and requires an empty
-- `PRAGMA foreign_key_check` before the commit; the `already_applied`
-- guard (`routine_runs_has_routine_name`) keeps a re-run from rebuilding.
CREATE TEMP TABLE routine_runs_before_166 AS SELECT * FROM routine_runs;

DROP TABLE routine_runs;

CREATE TABLE routine_runs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  routine_id     INTEGER REFERENCES routines(id) ON DELETE SET NULL,
  routine_name   TEXT,
  trigger        TEXT    NOT NULL CHECK (trigger IN ('cron', 'event', 'run_now')),
  trigger_ref    TEXT,
  state          TEXT    NOT NULL CHECK (state IN ('running', 'done', 'failed', 'skipped')),
  reason         TEXT,
  session_id     INTEGER,
  cost_micros    INTEGER NOT NULL DEFAULT 0,
  scheduled_for  INTEGER,
  started_at     INTEGER NOT NULL,
  finished_at    INTEGER,
  outcome        TEXT
    CHECK (outcome IN ('did_work', 'nothing', 'failed', 'needs_person')),
  outcome_source TEXT
    CHECK (outcome_source IN ('exit', 'rule', 'jev')),
  error_code     TEXT,
  host_alias     TEXT
);

INSERT INTO routine_runs
  (id, routine_id, routine_name, trigger, trigger_ref, state, reason, session_id,
   cost_micros, scheduled_for, started_at, finished_at, outcome, outcome_source,
   error_code, host_alias)
  SELECT b.id, b.routine_id, r.name, b.trigger, b.trigger_ref, b.state, b.reason,
         b.session_id, b.cost_micros, b.scheduled_for, b.started_at, b.finished_at,
         b.outcome, b.outcome_source, b.error_code, b.host_alias
    FROM routine_runs_before_166 b
    LEFT JOIN routines r ON r.id = b.routine_id;

DROP TABLE routine_runs_before_166;

CREATE INDEX IF NOT EXISTS routine_runs_by_routine ON routine_runs (routine_id, started_at);
CREATE INDEX IF NOT EXISTS routine_runs_running ON routine_runs (state) WHERE state = 'running';
CREATE INDEX IF NOT EXISTS routine_runs_started ON routine_runs (started_at);
CREATE INDEX IF NOT EXISTS routine_runs_session
  ON routine_runs (session_id) WHERE session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS routine_runs_unjudged
  ON routine_runs (finished_at, id)
  WHERE state = 'done' AND outcome IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (166);
