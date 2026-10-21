-- Routine memory: notebooks the fleet keeps for routines, outside any git
-- repo and any host's `~/.claude`, so they survive a fresh session, a
-- fallback host and a moved project. The rules are in
-- `service::routines::memory`.
--   routine_memory   one notebook per routine (`scope = 'routine'`,
--                    `owner_id` its routine) or per project (`scope =
--                    'project'`, `owner_id` its project, shared by every
--                    routine of that project). `updated_by`: who wrote it
--                    last, a person (the editor), a run (its reply's
--                    `<routine-memory>` block) or the fleet (a run's note).
--                    `circle`: whose notebook it is, so a project's is never
--                    shared past the people who share its routines: the
--                    routine's org id, else minus its owner's person id,
--                    else 0 (a one-person hub). A routine's own is 0.
CREATE TABLE IF NOT EXISTS routine_memory (
  scope          TEXT NOT NULL CHECK (scope IN ('routine', 'project')),
  owner_id       INTEGER NOT NULL,
  circle         INTEGER NOT NULL DEFAULT 0,
  body           TEXT NOT NULL DEFAULT '',
  updated_at     INTEGER NOT NULL,
  updated_by     TEXT NOT NULL CHECK (updated_by IN ('person', 'run', 'fleet')),
  updated_run_id INTEGER,
  PRIMARY KEY (scope, owner_id, circle)
);

-- routines
--   memory       which notebooks its runs read and may rewrite: routine |
--                project | both. NULL = none (the default).
--   memory_auto  1: the fleet adds one line per finished run (its state and
--                the first line of its reply) to the notebook.
ALTER TABLE routines ADD COLUMN memory TEXT CHECK (memory IN ('routine', 'project', 'both'));
ALTER TABLE routines ADD COLUMN memory_auto INTEGER NOT NULL DEFAULT 0;

-- routine_runs
--   memory_read_at  when its reply was read for the notebooks; NULL = not
--                   yet (or its routine keeps none).
ALTER TABLE routine_runs ADD COLUMN memory_read_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (204);
