-- Fleet events: what happened to the fleet rather than to one session, for
-- event routines (`service::fleet_events`, `service::routines::FLEET_EVENTS`).
--
-- `fleet_events` is written by `service::fleet_events`'s watcher when a
-- standing condition starts or ends (a host unreachable or back, low on
-- disk or memory; an account at its limit or under it again; a login
-- signed out), and by `Store::set_mission_state` when a mission completes
-- or fails. One row per change, never per pass: the watcher keeps what it
-- last saw and spaces a condition that flaps.
--   kind       a `FLEET_EVENTS` word (host_offline, account_limited, …)
--   subject    what it is about, a key a rate counts: `host:<alias>`,
--              `account:<uuid>`, `login:<host>:<profile>`, `mission:<id>`
--   detail     fleet's own words for it, never a session's
--   host_alias the host it is on, when it is on one (the host filter)
--   org_id     the org it belongs to, when one does
--   person_id  the person it is about (a mission's owner), when one is
-- `Store::insert_fleet_event` keeps the newest rows only.
CREATE TABLE IF NOT EXISTS fleet_events (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  at         INTEGER NOT NULL,
  kind       TEXT    NOT NULL,
  subject    TEXT    NOT NULL,
  detail     TEXT,
  host_alias TEXT,
  org_id     INTEGER,
  person_id  INTEGER
);

CREATE INDEX IF NOT EXISTS fleet_events_by_kind ON fleet_events (kind, id);

-- An event routine may narrow to one host (`event_host`): a host event of
-- that host, a session event of a session on it, a pull request event of a
-- PR seen there. NULL = any host.
ALTER TABLE routines ADD COLUMN event_host TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (201);
