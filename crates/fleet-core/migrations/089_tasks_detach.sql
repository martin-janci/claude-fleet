-- Multi-user M1 (T9d): a task outlives its sessions, and `sessions.id` is
-- REUSED.
--
-- `tasks.requester_session_id` / `worker_session_id` (migration 020) are
-- plain INTEGERs with no foreign key, and nothing in the tree ever deletes
-- or NULLs them — `delete_session`, the reconcile ghost reap, host removal
-- and project removal all clear `session_events` and retire `participants`
-- and say nothing about tasks. `sessions.id` is `INTEGER PRIMARY KEY` with
-- no AUTOINCREMENT (migration 001), so SQLite hands a reaped row's id to the
-- next session created on that host — and `task_visible_in_scope` resolved
-- both ends by that id, so a `task:updated` frame (and `list_tasks`) judged
-- another person's task against whoever holds its ids NOW, handing them the
-- task's `prompt` and `result`. Live, with no replay involved.
--
-- The answer is the shape 044 and 045 already chose for the same hazard: a
-- trigger, so none of the delete paths nor a future sixth can miss it, and
-- a stamp that says the identity is GONE rather than a NULL that would read
-- as "this task never had that end" — which `task_visible_in_scope_pure`
-- treats as "no claim to check" and would WIDEN. A detached task is the
-- hub's own business and nobody else's; its sessions are over, so there is
-- nothing left for a person to drive.
ALTER TABLE tasks ADD COLUMN detached_at INTEGER;

CREATE TRIGGER IF NOT EXISTS trg_tasks_on_session_delete
AFTER DELETE ON sessions
BEGIN
  UPDATE tasks
     SET requester_session_id =
           CASE WHEN requester_session_id = old.id THEN NULL
                ELSE requester_session_id END,
         worker_session_id =
           CASE WHEN worker_session_id = old.id THEN NULL
                ELSE worker_session_id END,
         detached_at = COALESCE(detached_at, CAST(strftime('%s', 'now') AS INTEGER))
   WHERE requester_session_id = old.id OR worker_session_id = old.id;
END;

-- Rows whose named session is already gone: de-identified here, once. The
-- upgrade widens nothing (rule 7) — every one of these is a task only the
-- hub reads from now on, where before it was readable by whoever happened to
-- hold the recycled id.
UPDATE tasks
   SET detached_at = CAST(strftime('%s', 'now') AS INTEGER)
 WHERE detached_at IS NULL
   AND ((requester_session_id IS NOT NULL
         AND requester_session_id NOT IN (SELECT id FROM sessions))
     OR (worker_session_id IS NOT NULL
         AND worker_session_id NOT IN (SELECT id FROM sessions)));
UPDATE tasks
   SET requester_session_id = NULL
 WHERE requester_session_id IS NOT NULL
   AND requester_session_id NOT IN (SELECT id FROM sessions);
UPDATE tasks
   SET worker_session_id = NULL
 WHERE worker_session_id IS NOT NULL
   AND worker_session_id NOT IN (SELECT id FROM sessions);

INSERT OR IGNORE INTO schema_version (version) VALUES (89);
