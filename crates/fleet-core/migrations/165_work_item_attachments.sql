-- Task attachments (the task page's Attachments section): files and images
-- people and agents add to a task, kept in fleet and never written to a
-- tracker. Like a comment, an attachment is about the ITEM, so it is fenced
-- with the item (its org), and deleting one is its author's alone.
--   item_id           the task; its attachments go with it
--   name              the file's name as given: no path, at most 200 chars
--   mime              its type, from a short allowlist (never SVG)
--   size              bytes; at most `work.attachment_max_mb`
--   sha256            hex digest of the bytes, computed by the store: the
--                     key into `work_attachment_blobs`
--   author            who added it as the hub names a caller (as a comment's)
--   author_person_id  the person behind it when the caller proves one
--   comment_id        the comment it was added with, if any
--   source            `fleet`; room for a tracker's own attachments later
--   external_id       that tracker's id for it
--   deleted_at        a deletion keeps the row but never serves it again
CREATE TABLE IF NOT EXISTS work_item_attachments (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id          INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  name             TEXT    NOT NULL,
  mime             TEXT    NOT NULL,
  size             INTEGER NOT NULL,
  sha256           TEXT    NOT NULL,
  author           TEXT    NOT NULL,
  author_person_id INTEGER,
  comment_id       INTEGER,
  source           TEXT    NOT NULL DEFAULT 'fleet',
  external_id      TEXT,
  created_at       INTEGER NOT NULL,
  deleted_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_work_item_attachments_item
  ON work_item_attachments(item_id, created_at);
CREATE INDEX IF NOT EXISTS idx_work_item_attachments_sha
  ON work_item_attachments(sha256);

-- The bytes, once per content: two attachments of the same file share one
-- blob, and the blob goes when its last live attachment does. In SQLite so
-- a hub backup carries them.
CREATE TABLE IF NOT EXISTS work_attachment_blobs (
  sha256     TEXT    PRIMARY KEY,
  bytes      BLOB    NOT NULL,
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (165);
