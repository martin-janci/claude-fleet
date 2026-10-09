-- Review r04 F3: whose file a Library item or a download is.
--
-- Once a session row is reaped, `service::library::visible` and
-- `service::downloads::visible` had only the row's org to go on, so the
-- index (and, for a download, the file's bytes) reached every person whose
-- org scope sees the host, not only the person who placed or sent it. The
-- owner is now recorded with the row, copied from the session when it is
-- written, and a reaped session's rows are the owner's alone.
--
-- Rows whose session is already gone keep NULL: they fall to the hub's own
-- readers and to a single-person hub's one person (`may_own_person_row`).
-- The ALTERs are not idempotent, so the entry is guarded.
ALTER TABLE library_items ADD COLUMN owner_person_id INTEGER;
ALTER TABLE downloads ADD COLUMN owner_person_id INTEGER;
UPDATE library_items
   SET owner_person_id = (SELECT owner_person_id FROM sessions
                           WHERE sessions.id = library_items.session_id)
 WHERE session_id IS NOT NULL;
UPDATE downloads
   SET owner_person_id = (SELECT owner_person_id FROM sessions
                           WHERE sessions.id = downloads.session_id)
 WHERE session_id IS NOT NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (148);
