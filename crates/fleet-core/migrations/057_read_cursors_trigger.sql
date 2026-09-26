-- Migration 044 was rewritten after it first landed: its early forms
-- created `read_cursors` without `trg_read_cursors_on_session_delete`, and
-- `migrate()` applies only versions above the recorded maximum, so a
-- database migrated by one of those builds recorded 44 and never gained the
-- trigger — a deleted session's cursors then outlive it, and a new session
-- handed the same id (`sessions.id` has no AUTOINCREMENT) inherits them: its
-- first read answers `unchanged` for data it never saw. No tagged release
-- carried the trigger-less file, so this closes it for the developer
-- databases that did. Same body as 044's; `IF NOT EXISTS`, safe to re-run.
CREATE TRIGGER IF NOT EXISTS trg_read_cursors_on_session_delete
AFTER DELETE ON sessions
BEGIN
  DELETE FROM read_cursors
   WHERE reader_session_id = old.id OR target_session_id = old.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (57);
