-- Design docs/superpowers/specs/2026-10-13-task-result-handover-design.md D4:
-- a follow-up task remembers the task whose result it came from. The origin
-- session, item and result all follow from that `tasks` row.
--
-- `work_items.origin_task_id`  NULL for every item not made from a result.
ALTER TABLE work_items ADD COLUMN origin_task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_origin_task
  ON work_items(origin_task_id) WHERE origin_task_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (195);
