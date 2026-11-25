-- A download and a library item outlive their session, and `sessions.id` is
-- REUSED (migration 001: INTEGER PRIMARY KEY, no AUTOINCREMENT).
--
-- `downloads.session_id` (095) and `library_items.session_id` (143) are
-- plain INTEGERs with no foreign key, and no delete path cleared them.
-- `downloads::visible` and `library::visible` judge a row by its live
-- session while that session exists, and by the owner and org recorded on
-- the row (migration 148) once it is gone: so once the next session took a
-- deleted session's id, whoever owned THAT session listed, fetched and
-- removed the previous owner's file, and the owner lost it.
--
-- The shape 101 chose for tasks: a trigger, so no delete path (nor a future
-- one) can miss it. NULL here means "the session is gone", which is the
-- recorded-owner branch both readers already have.
CREATE TRIGGER IF NOT EXISTS trg_downloads_library_on_session_delete
AFTER DELETE ON sessions
BEGIN
  UPDATE downloads SET session_id = NULL WHERE session_id = old.id;
  UPDATE library_items SET session_id = NULL WHERE session_id = old.id;
END;

-- Rows whose session is already gone, once: they may already point at a
-- session that reused the id, which cannot be told apart here, so only the
-- ones naming no session at all are detached. A row naming a live session
-- stays as it is.
UPDATE downloads SET session_id = NULL
 WHERE session_id IS NOT NULL AND session_id NOT IN (SELECT id FROM sessions);
UPDATE library_items SET session_id = NULL
 WHERE session_id IS NOT NULL AND session_id NOT IN (SELECT id FROM sessions);

INSERT OR IGNORE INTO schema_version (version) VALUES (231);
