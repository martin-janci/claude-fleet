-- File downloads (docs/superpowers/specs/2026-10-03-file-downloads-design.md):
-- a file a session's Claude (or a person) sent from a host, copied to this
-- machine's data dir (`<data dir>/downloads/<id>`) so a phone or desktop
-- can fetch it with `GET /downloads/<id>`.
--
-- session_id is not a foreign key: a download outlives its session, and
-- session_name / org_id are the snapshot the scope check and the list read
-- once the row is gone.
--
-- state   'fetching' | 'ready' | 'failed'
-- source  'agent' (a host's own token) | 'person'
CREATE TABLE IF NOT EXISTS downloads (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at            INTEGER NOT NULL,
  host_alias    TEXT    NOT NULL,
  session_id    INTEGER,
  session_name  TEXT,
  org_id        INTEGER,
  path          TEXT    NOT NULL,
  name          TEXT    NOT NULL,
  size          INTEGER NOT NULL,
  state         TEXT    NOT NULL DEFAULT 'fetching'
                CHECK (state IN ('fetching', 'ready', 'failed')),
  error         TEXT,
  sha256        TEXT,
  source        TEXT    NOT NULL,
  note          TEXT,
  ready_at      INTEGER,
  downloaded_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_downloads_at ON downloads(at);

INSERT OR IGNORE INTO schema_version (version) VALUES (95);
