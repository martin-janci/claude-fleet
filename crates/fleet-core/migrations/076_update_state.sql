-- Application updates (update-channel design §7.4, slice S4): the hub as a
-- desired-state controller for the fleet's own software.
--
-- `update_desired`: what the operator pinned, per component (target '') or
-- per target. `kind` is `artifact` for now; the same table is meant to carry
-- configuration / plugin / flag desired state later (design F8), which is
-- why it is part of the key.
CREATE TABLE IF NOT EXISTS update_desired (
  id         INTEGER PRIMARY KEY,
  kind       TEXT    NOT NULL DEFAULT 'artifact',
  component  TEXT    NOT NULL,
  target     TEXT    NOT NULL DEFAULT '',
  version    TEXT    NOT NULL,
  mandatory  INTEGER NOT NULL DEFAULT 0,
  reason     TEXT,
  set_by     TEXT    NOT NULL,
  set_at     INTEGER NOT NULL,
  UNIQUE (kind, component, target)
);

-- What each target last said about itself: the hub (`hub:self`), an agent
-- host (`agent:<alias>`), a paired client (`client:<id>`).
CREATE TABLE IF NOT EXISTS update_observed (
  target          TEXT    PRIMARY KEY,
  component       TEXT    NOT NULL,
  platform        TEXT,
  version         TEXT    NOT NULL,
  commit_sha      TEXT,
  build_id        TEXT,
  digest          TEXT,
  speaks          TEXT,
  phase           TEXT    NOT NULL DEFAULT 'idle',
  attempt         TEXT,
  last_error      TEXT,
  reported_at     INTEGER NOT NULL,
  last_checked_at INTEGER
);

-- The transition log. Idempotent on (target, attempt, phase): an updater
-- that replays its queued reports after the hub came back adds nothing twice.
CREATE TABLE IF NOT EXISTS update_events (
  id           INTEGER PRIMARY KEY,
  target       TEXT    NOT NULL,
  attempt      TEXT    NOT NULL DEFAULT '',
  phase        TEXT    NOT NULL,
  from_version TEXT,
  to_version   TEXT,
  detail       TEXT,
  error        TEXT,
  at           INTEGER NOT NULL,
  UNIQUE (target, attempt, phase)
);
CREATE INDEX IF NOT EXISTS idx_update_events_at ON update_events(at);

-- The signed documents the hub decides from, verbatim (the evidence it
-- relays): `channel` keyed by track, `manifest` keyed by version. Verified
-- again on every read; the cache is never trusted by itself.
CREATE TABLE IF NOT EXISTS update_docs (
  kind       TEXT    NOT NULL,
  key        TEXT    NOT NULL,
  body       TEXT    NOT NULL,
  sig        TEXT    NOT NULL,
  sequence   INTEGER,
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (kind, key)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (76);
