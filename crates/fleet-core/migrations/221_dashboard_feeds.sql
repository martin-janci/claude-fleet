-- Dashboard feeds (owner report 2026-11-06, "dashboards from dynamic
-- data"): a value a `feed` widget shows, written by Control's agent, the
-- person, or a routine's run, never computed by the app
-- (`service::dashboards`, `service::routines::memory`).
--
-- dashboard_feeds
--   owner       whose it is: a person id, 0 = fleet's own (one-person hub)
--   key         the name a `feed` widget shows it by
--   value       the text written last (at most 4000 characters)
--   by          who wrote it: person | agent | routine
--   by_ref      the routine's name, for a routine's write
--   updated_at  when, unix seconds
CREATE TABLE IF NOT EXISTS dashboard_feeds (
  owner      INTEGER NOT NULL DEFAULT 0,
  key        TEXT    NOT NULL,
  value      TEXT    NOT NULL,
  by         TEXT    NOT NULL CHECK (by IN ('person', 'agent', 'routine')),
  by_ref     TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, key)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (221);
