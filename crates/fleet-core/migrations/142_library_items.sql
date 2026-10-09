-- Control's Library (Orbit Fleet redesign 9.7): the files a person put on a
-- host, so the Library can list them beside the downloads (`downloads`,
-- migration 095) and the repos the fleet's sessions work in. One row per
-- file placed: `upload` (the Library's Upload…) or `attachment` (a file sent
-- with a prompt from the composer). The bytes stay on the host; the row is
-- the index.
--
-- session_id is not a foreign key, the same bargain as `downloads`: an item
-- outlives its session, and session_name / org_id are the snapshot the
-- scope check and the list read once the session row is gone.
CREATE TABLE IF NOT EXISTS library_items (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at            INTEGER NOT NULL,
  kind          TEXT    NOT NULL CHECK (kind IN ('upload', 'attachment')),
  host_alias    TEXT    NOT NULL,
  session_id    INTEGER,
  session_name  TEXT,
  org_id        INTEGER,
  path          TEXT    NOT NULL,
  name          TEXT    NOT NULL,
  size          INTEGER
);
CREATE INDEX IF NOT EXISTS idx_library_items_at ON library_items(at);

INSERT OR IGNORE INTO schema_version (version) VALUES (142);
