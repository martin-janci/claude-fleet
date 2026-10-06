-- Org administration phase D (multi-user M2): members and roles. Plan
-- docs/superpowers/plans/2026-10-06-org-administration-phase-d.md, spec
-- docs/superpowers/specs/2026-10-06-org-administration-design.md.
--
-- What this script does, statement by statement:
--
--   CREATE TABLE org_members           who is in which company, with a role
--   idx_org_members_person             a person's memberships, read per request
--   auth_epoch_org_members_*           a membership decides what a device
--                                      resolves to: every write re-resolves
--                                      every cached caller
--   ALTER TABLE orgs  owns_hub         the company that owns the hub
--   idx_orgs_owns_hub                  ... at most one
--   ALTER TABLE orgs  admins_see_unclaimed
--                                      the owner's per-org switch for the
--                                      unclaimed count (answer 3)
--
-- org_id, person_id  no foreign keys, the migration-066 rationale: a deleted
--               org or person must leave the row pointing at an id nothing
--               has (fail closed), never cascade it into a widening.
-- role          admin | member | viewer. The CHECK makes a fourth role a
--               migration rather than an accident.
-- added_at      when the person (last) joined.
-- added_by      the person who added them; NULL for the hub's operator
--               (fleet-hub) and the desktop's own store.
-- shares_since  when they last became able to receive what is shared with
--               the org (role member or admin); NULL while a viewer. An org
--               grant reaches a member only when it is not older than this:
--               changing a membership never widens an existing grant
--               (gap analysis §4.3, *Team sharing*).
-- removed_at    the person left the company. The row STAYS: a former
--               member's device reads nothing of any org rather than
--               falling back to every org's work.
CREATE TABLE IF NOT EXISTS org_members (
  org_id       INTEGER NOT NULL,
  person_id    INTEGER NOT NULL,
  role         TEXT    NOT NULL CHECK (role IN ('admin', 'member', 'viewer')),
  added_at     INTEGER NOT NULL,
  added_by     INTEGER,
  shares_since INTEGER,
  removed_at   INTEGER,
  PRIMARY KEY (org_id, person_id)
);

CREATE INDEX IF NOT EXISTS idx_org_members_person ON org_members(person_id);

CREATE TRIGGER IF NOT EXISTS auth_epoch_org_members_insert
  AFTER INSERT ON org_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

CREATE TRIGGER IF NOT EXISTS auth_epoch_org_members_update
  AFTER UPDATE ON org_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

CREATE TRIGGER IF NOT EXISTS auth_epoch_org_members_delete
  AFTER DELETE ON org_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

-- The company that owns the hub (owner's answer 1): its admins administer
-- hosts. Read per request, so it needs no auth-epoch trigger.
ALTER TABLE orgs ADD COLUMN owns_hub INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX IF NOT EXISTS idx_orgs_owns_hub
  ON orgs(owns_hub) WHERE owns_hub = 1;

-- Owner's answer 3: an org's admins see the count of unclaimed sessions on
-- the org's hosts only when the hub's owner turns this on. Off by default.
ALTER TABLE orgs ADD COLUMN admins_see_unclaimed INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (105);
