-- Mission improvements M1 (docs/superpowers/specs/2026-10-26-mission-
-- improvements-design.md §2): what a lane's tasks cost when they share
-- one lead session.
--
-- tasks
--   cost_start_micros  the lead session's `usage_cost_micros` when the
--                      task was handed to it. A task's cost is the next
--                      task's start minus its own, the last one's the
--                      session's spend minus its own. NULL for a run in a
--                      session of its own, which keeps the whole session's
--                      spend. -1: a lane task from before this column, its
--                      session's spend split evenly among its tasks.
ALTER TABLE tasks ADD COLUMN cost_start_micros INTEGER;

UPDATE tasks SET cost_start_micros = -1 WHERE start_head IS NOT NULL;

-- The Work view reads every such task on each load: a partial index keeps
-- that read to the lane tasks.
CREATE INDEX IF NOT EXISTS idx_tasks_cost_start
  ON tasks(worker_session_id, id) WHERE cost_start_micros IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (213);
