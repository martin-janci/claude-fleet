-- Work graph M3 (docs/superpowers/plans/2026-09-24-work-graph-m3-trackers-and-jira.md):
-- trackers, their secrets and views, and the tracker attributes of work_items.
--
-- trackers         one external tracker site (Jira Cloud in M3). `state` is
--                  ok | auth_failed | rate_limited | unreachable | captcha |
--                  unconfigured; `last_error` is redacted before it is stored.
--                  `config` is JSON: account_id, tz, key_prefixes, has_sprints
--                  per project, disabled views.
-- tracker_secrets  the credential, in its own table so no read path can
--                  select it by accident: `value` (stored) or `credential_ref`
--                  (`env:NAME` | `file:/run/secrets/x`, read at use). Only
--                  `store::trackers::resolve_credential` reads this table.
-- tracker_views    the queries a sync runs (built-in `mine`/`sprint`/`recent`
--                  and favourite filters), each with its own time watermark.
-- work_items +=    identity stays `(tracker_id, external_id)` (C24): the key
--                  and its former keys (`aliases`, JSON) are attributes, so a
--                  moved issue keeps its links. `unavailable_at` /
--                  `unavailable_reason` record "missing" (C25) — nothing is
--                  ever deleted because of a tracker's answer.
CREATE TABLE IF NOT EXISTS trackers (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  provider      TEXT    NOT NULL,
  name          TEXT    NOT NULL,
  instance_id   TEXT,
  site_url      TEXT    NOT NULL,
  api_base      TEXT,
  org_id        INTEGER,
  transport     TEXT    NOT NULL DEFAULT 'direct',
  config        TEXT,
  state         TEXT    NOT NULL DEFAULT 'unconfigured',
  last_sync_at  INTEGER,
  last_error    TEXT,
  created_at    INTEGER NOT NULL,
  UNIQUE(provider, site_url)
);

CREATE TABLE IF NOT EXISTS tracker_secrets (
  tracker_id     INTEGER PRIMARY KEY REFERENCES trackers(id) ON DELETE CASCADE,
  auth_kind      TEXT    NOT NULL,
  username       TEXT,
  value          TEXT,
  credential_ref TEXT
);

CREATE TABLE IF NOT EXISTS tracker_views (
  tracker_id  INTEGER NOT NULL REFERENCES trackers(id) ON DELETE CASCADE,
  view_id     TEXT    NOT NULL,
  label       TEXT    NOT NULL,
  query       TEXT    NOT NULL,
  watermark   INTEGER,
  enabled     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (tracker_id, view_id)
);

ALTER TABLE work_items ADD COLUMN aliases TEXT;
ALTER TABLE work_items ADD COLUMN kind TEXT;
ALTER TABLE work_items ADD COLUMN hierarchy_level INTEGER;
ALTER TABLE work_items ADD COLUMN status_name TEXT;
ALTER TABLE work_items ADD COLUMN resolution TEXT;
ALTER TABLE work_items ADD COLUMN parent_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN containers TEXT;
ALTER TABLE work_items ADD COLUMN assignees TEXT;
ALTER TABLE work_items ADD COLUMN iteration TEXT;
ALTER TABLE work_items ADD COLUMN meta TEXT;
ALTER TABLE work_items ADD COLUMN updated_ext INTEGER;
ALTER TABLE work_items ADD COLUMN status_changed_at INTEGER;
ALTER TABLE work_items ADD COLUMN fetched_at INTEGER;
ALTER TABLE work_items ADD COLUMN unavailable_at INTEGER;
ALTER TABLE work_items ADD COLUMN unavailable_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_work_items_key ON work_items(key);
CREATE INDEX IF NOT EXISTS idx_work_items_tracker ON work_items(tracker_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (48);
