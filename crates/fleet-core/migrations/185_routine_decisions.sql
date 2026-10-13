-- Routine debugging: what the scheduler decided for a routine and why.
--
-- `routine_runs` holds a fire that started a run or was skipped. A timeline
-- event an event routine read and did not act on (not its owner's session,
-- a session a routine started, the repo filter, the rate, a busy routine)
-- left no trace, so "why did it not fire?" had no answer. Each decision is
-- a row here: `fired`, `failed` (its start failed), `skipped` (a run row
-- says why), `dropped` (an event fire the routine was busy, over budget or
-- inside its rate for) and `filtered` (an event that was not the routine's).
-- `Store::insert_routine_decision` keeps the newest 200 per routine, so a
-- frequent event kind cannot grow it without bound.
CREATE TABLE IF NOT EXISTS routine_decisions (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  routine_id  INTEGER NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
  at          INTEGER NOT NULL,
  trigger     TEXT    NOT NULL,
  verdict     TEXT    NOT NULL
    CHECK (verdict IN ('fired', 'failed', 'skipped', 'dropped', 'filtered')),
  reason      TEXT,
  trigger_ref TEXT,
  event_id    INTEGER,
  session_id  INTEGER,
  run_id      INTEGER
);

CREATE INDEX IF NOT EXISTS routine_decisions_by_routine
  ON routine_decisions (routine_id, id);

INSERT OR IGNORE INTO schema_version (version) VALUES (185);
