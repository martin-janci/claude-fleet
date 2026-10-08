-- Routines (Orbit Fleet redesign step 8.5): a saved prompt that starts a
-- session on a schedule, on a fleet event, or when a person presses Run
-- now. The rules (who may see and change one, the cron grammar, the
-- scheduler tick, budgets and the overlap rule) are in `service::routines`;
-- these are the rows.
--
-- routines
--   org_id            the org it belongs to; NULL = unassigned. The org's
--                     removal leaves it unassigned, as a mission's does.
--   owner_person_id   whose it is: its runs' sessions are this person's
--   trigger           cron | event | manual (manual = Run now only)
--   cron              five fields, minute hour day-of-month month weekday,
--                     read at utc_offset_min (minutes east of UTC)
--   event             a session timeline kind (`service::routines::EVENTS`)
--   event_cursor      the newest session_events.id already looked at, so
--                     an event fires a routine once and history never does
--   host_alias, project_id, profile
--                     where its session starts and the credential profile
--                     (the account) it bills; profile NULL = the host's own
--   budget_run_micros a run past it is failed and the routine paused
--   budget_day_micros no new run once its runs spent this today
--   overlap           skip (no run while one is open) | parallel
--   next_run_at       the next cron fire (unix seconds); NULL for the rest
--   skip_next         the next scheduled fire is recorded as skipped
--   paused_reason     why the scheduler turned it off, for a person
--   lease_until       one tick at a time, as a mission's lease
CREATE TABLE IF NOT EXISTS routines (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id            INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  owner_person_id   INTEGER,
  name              TEXT    NOT NULL,
  enabled           INTEGER NOT NULL DEFAULT 1,
  trigger           TEXT    NOT NULL CHECK (trigger IN ('cron', 'event', 'manual')),
  cron              TEXT,
  utc_offset_min    INTEGER NOT NULL DEFAULT 0,
  event             TEXT,
  event_cursor      INTEGER NOT NULL DEFAULT 0,
  host_alias        TEXT    NOT NULL,
  project_id        INTEGER NOT NULL,
  profile           TEXT,
  prompt            TEXT    NOT NULL,
  budget_run_micros INTEGER,
  budget_day_micros INTEGER,
  overlap           TEXT    NOT NULL DEFAULT 'skip' CHECK (overlap IN ('skip', 'parallel')),
  next_run_at       INTEGER,
  skip_next         INTEGER NOT NULL DEFAULT 0,
  paused_reason     TEXT,
  lease_until       INTEGER,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS routines_due ON routines (enabled, next_run_at);
CREATE INDEX IF NOT EXISTS routines_owner ON routines (owner_person_id);

-- routine_runs: one row per fire, skipped ones included, so Runs (8.3) and
-- the Inbox (8.6) can say what happened and why nothing did.
--   trigger      cron | event | run_now
--   trigger_ref  the event that fired it (`session:<id>:<event id>`)
--   state        running | done | failed | skipped
--   session_id   the session it started; no foreign key, the run outlives
--                a killed session
--   cost_micros  what its session spent, refreshed while it runs
CREATE TABLE IF NOT EXISTS routine_runs (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  routine_id    INTEGER NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
  trigger       TEXT    NOT NULL CHECK (trigger IN ('cron', 'event', 'run_now')),
  trigger_ref   TEXT,
  state         TEXT    NOT NULL CHECK (state IN ('running', 'done', 'failed', 'skipped')),
  reason        TEXT,
  session_id    INTEGER,
  cost_micros   INTEGER NOT NULL DEFAULT 0,
  scheduled_for INTEGER,
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER
);
CREATE INDEX IF NOT EXISTS routine_runs_by_routine ON routine_runs (routine_id, started_at);
CREATE INDEX IF NOT EXISTS routine_runs_running ON routine_runs (state) WHERE state = 'running';

INSERT OR IGNORE INTO schema_version (version) VALUES (130);
