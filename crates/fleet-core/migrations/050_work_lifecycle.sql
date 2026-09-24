-- Work graph M7 (docs/superpowers/plans/2026-09-24-work-graph-m7-self-cleaning-lifecycle.md):
-- the self-cleaning lifecycle. Nothing here deletes anything.
--
-- work_links.archived_at   a LIVE session archived from the UI: it collapses
--                          into its work group's Done section while tmux keeps
--                          running. The next prompt or attach clears it.
--                          (Past work stays `ended_at`, the M1b trigger.)
-- work_links.tidy_snoozed_until
--                          Tidy-up does not suggest this link's session before
--                          then ("Snooze 7 d").
-- work_links.tidy_never    1: Tidy-up never suggests it ("Never for this work").
-- sessions.last_touch_at   the last prompt (UserPromptSubmit or a fleet send)
--                          or attach: a session touched within the hour is
--                          never a tidy candidate. Outside reconcile's
--                          `ON CONFLICT` list, like 049's columns.
-- work_items.reopened_at   the item moved out of `done` (a tracker transition):
--                          the Attention "Reopened" entry, until the work is
--                          resumed, the item is done again, or it is dismissed.
ALTER TABLE work_links ADD COLUMN archived_at INTEGER;
ALTER TABLE work_links ADD COLUMN tidy_snoozed_until INTEGER;
ALTER TABLE work_links ADD COLUMN tidy_never INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_touch_at INTEGER;
ALTER TABLE work_items ADD COLUMN reopened_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (50);
