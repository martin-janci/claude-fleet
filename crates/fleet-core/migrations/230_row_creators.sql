-- 230: who made a row, kept for good (rounds 2 and 3 of the 2026-11-23
-- reviews).
--
-- pinned_dashboards
--   created_by         the device that pinned the row first; never changed
--                      by a replacement or an undo. Undo removes a dashboard
--                      only when Control's agent made it (review w14-1: the
--                      current `device` named the wrong author after the
--                      person reverted Control's change themselves). Filled
--                      only for a row never replaced since its pin; NULL =
--                      not known, which never reads as Control's.
-- control_tokens
--   created_by_person  the person whose device created the named token;
--                      NULL = the master token or the hub's operator. A
--                      person disabled takes the live tokens they created
--                      with them (review w11-1).
ALTER TABLE pinned_dashboards ADD COLUMN created_by TEXT;
UPDATE pinned_dashboards SET created_by = device
 WHERE previous_spec IS NULL AND pinned_at = updated_at;

ALTER TABLE control_tokens ADD COLUMN created_by_person INTEGER REFERENCES people(id);

INSERT OR IGNORE INTO schema_version (version) VALUES (230);
