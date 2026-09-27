-- Live-instance analysis 2026-09-27, lifecycle F2: two `working` rows had
-- had no Stop, no transcript growth and no pane output for ~40 h, and
-- nothing could demote them. The tick now turns such a row `idle` after
-- `reconcile.stale_working_secs`, and this stamp says it did (attention
-- reason `stale_working`). Cleared by the next UserPromptSubmit / Stop /
-- StopFailure / SessionEnd / Notification hook, and by a pane that shows a
-- live turn; the cached `claude agents` status alone does not lift it.
ALTER TABLE sessions ADD COLUMN stale_working_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (63);
