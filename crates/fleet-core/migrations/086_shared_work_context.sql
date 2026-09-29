-- Shared work context (design 2026-09-29, AI task system roadmap part 1).
--
-- `origin`         manual | proposed | agent | detected. NULL reads as
--                  `detected` (a row an older hub wrote).
-- `project_id`     where a native item starts.
-- `notes`          a native item's brief (≤ BRIEF_MAX_CHARS, cut by the writer).
-- `task_id`        the dispatched job (`tasks`) an `agent` item mirrors.
-- `proposal_state` proposed | accepted | rejected, for `origin = 'proposed'`.
-- `proposed_by`    who proposed it (a session label).
-- `proposal_why`   the proposer's reason (≤ BRIEF_MAX_CHARS).
--
-- Backfill: every local item existing today was named through "Name this
-- work…" (manual); every other row came from a tracker (detected).
ALTER TABLE work_items ADD COLUMN origin         TEXT;
ALTER TABLE work_items ADD COLUMN project_id     INTEGER REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN notes          TEXT;
ALTER TABLE work_items ADD COLUMN task_id        INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN proposal_state TEXT;
ALTER TABLE work_items ADD COLUMN proposed_by    TEXT;
ALTER TABLE work_items ADD COLUMN proposal_why   TEXT;

UPDATE work_items SET origin = CASE WHEN source = 'local' THEN 'manual' ELSE 'detected' END
 WHERE origin IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_task ON work_items(task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_parent ON work_items(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_proposals ON work_items(parent_id) WHERE proposal_state = 'proposed';

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
