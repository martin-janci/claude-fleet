-- Task comments (canvas gap "Task detail tabs … Comments", the task page's
-- Comments tab): people and agents leave a note on a task, kept in fleet and
-- never written to a tracker. A comment is about the ITEM, so it is fenced
-- with the item (its org), and deleting one is its author's alone.
--   item_id           the task; the comments go with it
--   author            who wrote it as the hub names a caller: `client:<name>`,
--                     `host:<alias>`, `master`, `desktop` on a standalone app
--   author_person_id  the person behind it when the caller proves one; what
--                     "mine" and deletion compare first
--   body              plain text, at most 4000 characters, rendered as text
--   deleted_at        a deletion keeps the row (a thread does not silently
--                     renumber) but it is never served again
CREATE TABLE IF NOT EXISTS work_item_comments (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id          INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  author           TEXT    NOT NULL,
  author_person_id INTEGER,
  body             TEXT    NOT NULL,
  created_at       INTEGER NOT NULL,
  deleted_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_work_item_comments_item
  ON work_item_comments(item_id, created_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (161);
