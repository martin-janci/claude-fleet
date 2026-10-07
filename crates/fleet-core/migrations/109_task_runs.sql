-- Orchestration O0 (docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md
-- §4.4): one work item, many attempts.
--
-- `work_items.task_id` (086) is 1:1 and means "this item MIRRORS that
-- dispatched job" (origin 'agent'). A run of an item that already exists
-- (`work_link { run }`) points the other way instead, so an item can be
-- attempted more than once, by more than one role, without a mirror item:
--
-- `tasks.work_item_id`  the item this task is an attempt at (NULL: a plain
--                       dispatch, as before).
-- `tasks.attempt`       1, 2, … within (work_item_id, role).
-- `tasks.role`          implement | review | test | research | integrate.
ALTER TABLE tasks ADD COLUMN work_item_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN attempt INTEGER;
ALTER TABLE tasks ADD COLUMN role TEXT;

CREATE INDEX IF NOT EXISTS idx_tasks_item
  ON tasks(work_item_id, created_at DESC) WHERE work_item_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (109);
