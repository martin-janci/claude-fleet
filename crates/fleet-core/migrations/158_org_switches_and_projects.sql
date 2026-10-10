-- Org forms (Orbit Fleet M15 step G2.10): the "Members see only their own
-- sessions" switch, and the org's project catalog.
--
--   ALTER TABLE orgs  members_own_sessions_only
--               1 (the default, and every org before this migration): a
--               member sees their own sessions and what is shared with them,
--               as before. 0: the org's members also WATCH each other's
--               sessions in it (`ViewScope`'s team reach) — read only, never
--               answer or drive, and only sessions whose owner is a live
--               member of the org too. Read per request, so it needs no
--               auth-epoch trigger.
--
--   CREATE TABLE org_projects   org-wide projects (a project catalog entry):
--     name      what the org calls it; unique within the org, without case.
--     remote    its git remote, if any.
--     path      where it is checked out on the hosts, if the same on each.
--     hosts     the hosts it may run on, comma-separated aliases; '' = every
--               host of the org.
--   org_id has no foreign key, the migration-066 rationale: a removed org's
--   rows are deleted by `Store::remove_org`, never cascaded by SQLite.
ALTER TABLE orgs ADD COLUMN members_own_sessions_only INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS org_projects (
  id         INTEGER PRIMARY KEY,
  org_id     INTEGER NOT NULL,
  name       TEXT    NOT NULL,
  remote     TEXT,
  path       TEXT,
  hosts      TEXT    NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_org_projects_name
  ON org_projects(org_id, name COLLATE NOCASE);

INSERT OR IGNORE INTO schema_version (version) VALUES (158);
