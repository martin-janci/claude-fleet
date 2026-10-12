-- Orbit Fleet M15 (G1.7): a task's due date. The Board card says who has it
-- and when it is due ("You · Fri"); the task detail edits it.
--
-- `work_items.due_at`  the calendar date the work is due, `YYYY-MM-DD`, no
--                      time and no zone (a due DATE is the same day
--                      everywhere). A person sets it on a native item
--                      (`work_link { edit | create, due_at }`); a tracker's
--                      sync writes its own (Jira `duedate`). NULL = no date.
--
-- Assignees already live in `work_items.assignees` (migration 048).
ALTER TABLE work_items ADD COLUMN due_at TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (154);
