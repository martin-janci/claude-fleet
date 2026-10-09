-- Review r02 (migrations): two repairs for routines (migration 131).
--
-- 1. Routines whose project or host is gone. `routines` names its project
--    and host without a foreign key, and until review r02 neither deleting
--    a project nor removing a host touched its routines, so an enabled one
--    kept firing on schedule, and each fire failed ("missing project") and
--    wrote a failed run. Such a routine stops now with the reason; the
--    delete paths do the same from here on (`Store::delete_project`,
--    `Store::delete_host`). `local` is never removed, and a hub without a
--    `local` row may still run a routine there, so it is left alone.
UPDATE routines
   SET enabled = 0,
       paused_reason = 'paused: its project was removed',
       next_run_at = NULL,
       updated_at = CAST(strftime('%s', 'now') AS INTEGER)
 WHERE enabled = 1
   AND project_id NOT IN (SELECT id FROM projects);
UPDATE routines
   SET enabled = 0,
       paused_reason = 'paused: its host was removed',
       next_run_at = NULL,
       updated_at = CAST(strftime('%s', 'now') AS INTEGER)
 WHERE enabled = 1
   AND host_alias <> 'local'
   AND host_alias NOT IN (SELECT alias FROM hosts);

-- 2. The finished runs Jev has not judged yet (8.10): the scheduler reads
--    them every tick (`recent_routine_runs_without_outcome`), and no index
--    covered `state = 'done' AND outcome IS NULL ... ORDER BY finished_at`,
--    so each tick scanned the whole history. A partial index holds only
--    the runs still waiting, so it stays small.
CREATE INDEX IF NOT EXISTS routine_runs_unjudged
  ON routine_runs (finished_at, id)
  WHERE state = 'done' AND outcome IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (146);
