-- Native item status (design 2026-09-28 §2): who decided this item's status.
--
-- `status_category` keeps its three values and stays the stored column; these
-- two say where the value came from, which is what makes the precedence rule
-- expressible: a person outranks a derived stamp, and a derived stamp outranks
-- the live signal.
--
-- `status_set_by`: NULL (nobody — the value is the sync's or the default) |
-- 'person' (an explicit setting, final) | 'derived' (stamped once by the tidy
-- pass that saw a merged PR; stamped rather than computed because
-- `sessions.pr_signals` dies with its session, and a computed `done` would
-- silently revert to `todo` afterwards).
ALTER TABLE work_items ADD COLUMN status_set_by TEXT;
ALTER TABLE work_items ADD COLUMN status_set_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_work_items_status_set
  ON work_items(status_set_by) WHERE status_set_by IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (75);
