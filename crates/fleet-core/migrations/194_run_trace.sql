-- Run traces: what happened inside one run, step by step.
--
-- A run row (`routine_runs`, `agent_jobs`, …) holds its start, its end and
-- one outcome; nothing between. Each step of a run is a row here, so the
-- Automation screen can draw a run as it goes (trigger, the checks it
-- passed, where it was placed, its session starting, its prompt typed, the
-- work, how it settled and who judged its outcome) and a person or Jev can
-- read why it went the way it did.
--
-- `run_ref` is the run's `runs { list }` id (`routine:418`, `agent:91`);
-- `seq` counts its steps from 1. `attempt` is the try a step belongs to (an
-- agent job retries in its own row); `parent_ref` links a retry's run to
-- the run it retries. `data` is a JSON object of at most 4 KB, never a
-- secret. `Store::insert_run_trace` caps a run's rows and drops rows past
-- `RUN_TRACE_KEEP_SECS`, so the table cannot grow without bound.
CREATE TABLE IF NOT EXISTS run_trace (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  run_ref      TEXT    NOT NULL,
  seq          INTEGER NOT NULL,
  attempt      INTEGER NOT NULL DEFAULT 1,
  parent_ref   TEXT,
  at           INTEGER NOT NULL,
  phase        TEXT    NOT NULL,
  kind         TEXT    NOT NULL,
  actor        TEXT    NOT NULL,
  summary      TEXT    NOT NULL,
  data         TEXT,
  duration_ms  INTEGER,
  cost_micros  INTEGER,
  decision_id  INTEGER,
  UNIQUE (run_ref, seq)
);

CREATE INDEX IF NOT EXISTS run_trace_by_at ON run_trace (at);

INSERT OR IGNORE INTO schema_version (version) VALUES (194);
