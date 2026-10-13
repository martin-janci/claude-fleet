-- "Finished — close?" (CF-5): a person's "Keep open" on local work whose
-- sessions have all ended.
--
-- The Work view suggests closing a local item once nothing runs on it. A
-- person who says "keep open" is not setting a status — `status_set_by =
-- 'person'` is final and would stop the item ever reading `in_progress` or
-- being stamped `done` by a merged PR — so the answer has its own row: the
-- suggestion stays away until a session ends on the item AFTER
-- `dismissed_at`.
CREATE TABLE IF NOT EXISTS work_close_dismissals (
  item_id      INTEGER PRIMARY KEY REFERENCES work_items(id) ON DELETE CASCADE,
  dismissed_at INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (184);
