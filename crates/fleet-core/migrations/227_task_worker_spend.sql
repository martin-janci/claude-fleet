-- A mission's spend outlives its workers' session rows (review R5).
--
-- A mission's spend was the usage of the sessions its tasks still name
-- (`tasks.worker_session_id`). A session row is deleted when it is killed
-- and reaped (a lane relay, the loop's own cancel, Clean up), and 101's
-- trigger then NULLs the id, so that session's whole spend left the grant
-- budget, the per-start check and the run estimate: a lane mission that
-- relays every few tasks never reached its brake.
--
-- tasks
--   worker_spent_micros  what the task's worker session had spent when its
--                        row was deleted. Written once per session and
--                        mission it served, on the newest of its tasks on
--                        that mission's items (not a wrapup when it has
--                        another), so a lead with several tasks is counted
--                        once by its mission, and a later task outside the
--                        mission (a dispatched task, another mission's run)
--                        does not take the mission's share; its tasks on no
--                        mission keep one copy the same way. Its other tasks
--                        get 0. NULL while the session row lives (its spend
--                        is read live, once per mission that counts it).
--
-- BEFORE DELETE, so the tasks still name the session: 101's AFTER DELETE
-- trigger NULLs the id.
ALTER TABLE tasks ADD COLUMN worker_spent_micros INTEGER;

CREATE TRIGGER IF NOT EXISTS trg_tasks_keep_worker_spend
BEFORE DELETE ON sessions
BEGIN
  UPDATE tasks
     SET worker_spent_micros = COALESCE(worker_spent_micros, 0) +
           CASE WHEN id IN (
                  SELECT (SELECT t.id FROM tasks t
                            LEFT JOIN work_items i ON i.id = t.work_item_id
                           WHERE t.worker_session_id = old.id
                             AND i.orchestration_project_id IS m.mission
                           ORDER BY COALESCE(t.role, '') = 'wrapup', t.id DESC
                           LIMIT 1)
                    FROM (SELECT DISTINCT i.orchestration_project_id AS mission
                            FROM tasks t
                            LEFT JOIN work_items i ON i.id = t.work_item_id
                           WHERE t.worker_session_id = old.id) m)
                THEN COALESCE(old.usage_cost_micros, 0) ELSE 0 END
   WHERE worker_session_id = old.id;
END;

-- A removed project's routines let go of its id (review R6). A project
-- id is a reused rowid: the next project added took it, and Resume started
-- the routine in that other repository. Each routine stopped for its
-- removed project, and each still naming a project that is gone, points at
-- 0 (`ROUTINE_NO_PROJECT`, never a rowid), stays off with the reason, and
-- needs a project picked again.
UPDATE routines
   SET project_id = 0, enabled = 0, next_run_at = NULL,
       paused_reason = 'paused: its project was removed'
 WHERE paused_reason = 'paused: its project was removed'
    OR project_id NOT IN (SELECT id FROM projects);

INSERT OR IGNORE INTO schema_version (version) VALUES (227);
