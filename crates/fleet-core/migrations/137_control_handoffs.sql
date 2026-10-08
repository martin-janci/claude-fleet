-- 137: `control_handoffs`, the receipts of what Control's agent (the
-- operator session) sent where (Orbit Fleet redesign step 9.3,
-- docs/ux/2026-10-08-orbit-fleet-redesign/transition-plan.md). One row per
-- successful call of the operator that handed work on: a prompt to a
-- session, a dispatched task, a new session, a mission saved or started, a
-- task created, a tree of subtasks proposed (9.6). Control draws each as a
-- chip or card that follows its target's live state.
--
-- kind        session | mission | task | tree
-- tool        the control-API tool that made it (send_prompt, work_link, …)
-- session_id  kind session: the session it went to. No FK: a killed
--             session keeps its receipt, which then says so.
-- task_id     dispatch_task's `tasks` row
-- mission_id  kind mission: the mission
-- item_id     kind task: the created item; kind tree: the parent item
-- item_ids    kind tree: the proposed items, a JSON array of ids
-- preview     the first line of what was sent, at most 160 characters
--
-- New objects only, `IF NOT EXISTS`, safe to re-run.
CREATE TABLE IF NOT EXISTS control_handoffs (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  at         INTEGER NOT NULL,
  kind       TEXT    NOT NULL CHECK (kind IN ('session', 'mission', 'task', 'tree')),
  tool       TEXT    NOT NULL,
  session_id INTEGER,
  task_id    INTEGER,
  mission_id INTEGER,
  item_id    INTEGER,
  item_ids   TEXT,
  preview    TEXT
);

INSERT OR IGNORE INTO schema_version (version) VALUES (137);
