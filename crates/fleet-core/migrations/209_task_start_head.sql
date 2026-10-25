-- Lanes L3 (docs/superpowers/specs/2026-10-23-mission-lanes-design.md
-- §3.3): where a task began in a checkout it shares with the tasks before
-- it.
--
-- tasks
--   start_head  the commit the lane's branch stood on when the task was
--               handed to its lead session; the task's evidence is
--               start_head..HEAD at its end. NULL for a run in a checkout
--               of its own, whose evidence is read against the merge base
--               as before.
ALTER TABLE tasks ADD COLUMN start_head TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (209);
