-- Identity sub-project 1 (spec docs/superpowers/specs/2026-10-11-identity-realm-model-design.md §1-2):
-- the realm model. Groups and role mappings are the source of truth; from
-- here on `org_members` is a projection of them, written only by
-- `Store::reproject_org_memberships` (store/realm.rs).
--
-- No foreign key to people or orgs (the migration-066 rationale): a row
-- pointing at a removed id fails closed. Keys between the new tables do
-- cascade, because removing a role or a group removes what it grants.
-- `uuid` columns are filled by `Store::backfill_realm_uuids`.

CREATE TABLE IF NOT EXISTS clients (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT UNIQUE,
  client_id  TEXT NOT NULL UNIQUE,
  name       TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS roles (
  id          INTEGER PRIMARY KEY,
  uuid        TEXT UNIQUE,
  client_id   INTEGER REFERENCES clients(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  description TEXT,
  seeded      INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_roles_name ON roles(ifnull(client_id, 0), name);

CREATE TABLE IF NOT EXISTS role_composites (
  parent_id INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  child_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (parent_id, child_id),
  CHECK (parent_id <> child_id)
);

-- org_id  set on an org's root group `/orgs/<org>` only.
-- managed 1 for `/orgs`, every org root and its three subgroups: their
--         structure follows the org; only membership is edited.
CREATE TABLE IF NOT EXISTS groups (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT UNIQUE,
  name       TEXT NOT NULL,
  parent_id  INTEGER REFERENCES groups(id) ON DELETE CASCADE,
  attributes TEXT NOT NULL DEFAULT '{}',
  managed    INTEGER NOT NULL DEFAULT 0,
  org_id     INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_name ON groups(ifnull(parent_id, 0), name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_org ON groups(org_id) WHERE org_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS group_members (
  group_id  INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL,
  added_at  INTEGER NOT NULL,
  added_by  INTEGER,
  PRIMARY KEY (group_id, person_id)
);
CREATE INDEX IF NOT EXISTS idx_group_members_person ON group_members(person_id);

CREATE TABLE IF NOT EXISTS user_role_mappings (
  person_id INTEGER NOT NULL,
  role_id   INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (person_id, role_id)
);

CREATE TABLE IF NOT EXISTS group_role_mappings (
  group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  role_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, role_id)
);


INSERT OR IGNORE INTO clients (client_id, name, created_at) VALUES ('fleet', 'claude-fleet', strftime('%s','now'));

INSERT OR IGNORE INTO roles (client_id, name, description, seeded, created_at) VALUES
  (NULL, 'fleet-admin',   'Administers the whole fleet', 1, strftime('%s','now')),
  (NULL, 'fleet-trusted', 'Their full devices may change settings and administer', 1, strftime('%s','now')),
  (NULL, 'assets-admin',  'Manages the personal asset catalog', 1, strftime('%s','now'));
INSERT OR IGNORE INTO roles (client_id, name, description, seeded, created_at)
  SELECT c.id, v.name, v.descr, 1, strftime('%s','now')
  FROM clients c, (SELECT 'org-admin' AS name, 'Administers an org' AS descr
                   UNION ALL SELECT 'org-member', 'Works in an org'
                   UNION ALL SELECT 'org-viewer', 'Reads an org') v
  WHERE c.client_id = 'fleet';
INSERT OR IGNORE INTO role_composites (parent_id, child_id)
  SELECT p.id, ch.id FROM roles p JOIN roles ch ON ch.client_id = p.client_id
  JOIN clients c ON c.id = p.client_id AND c.client_id = 'fleet'
  WHERE (p.name, ch.name) IN (VALUES ('org-admin','org-member'), ('org-member','org-viewer'));

INSERT OR IGNORE INTO groups (name, parent_id, managed, created_at) VALUES ('orgs', NULL, 1, strftime('%s','now'));
INSERT OR IGNORE INTO groups (name, parent_id, managed, org_id, created_at)
  SELECT o.name, (SELECT id FROM groups WHERE parent_id IS NULL AND name = 'orgs'), 1, o.id, strftime('%s','now')
  FROM orgs o;
INSERT OR IGNORE INTO groups (name, parent_id, managed, created_at)
  SELECT s.name, g.id, 1, strftime('%s','now')
  FROM groups g, (SELECT 'admins' AS name UNION ALL SELECT 'members' UNION ALL SELECT 'viewers') s
  WHERE g.org_id IS NOT NULL;
INSERT OR IGNORE INTO group_role_mappings (group_id, role_id)
  SELECT sg.id, r.id FROM groups sg JOIN groups root ON root.id = sg.parent_id AND root.org_id IS NOT NULL
  JOIN roles r ON r.name = CASE sg.name WHEN 'admins' THEN 'org-admin' WHEN 'members' THEN 'org-member' ELSE 'org-viewer' END
  JOIN clients c ON c.id = r.client_id AND c.client_id = 'fleet';

INSERT OR IGNORE INTO group_members (group_id, person_id, added_at, added_by)
  SELECT sg.id, m.person_id, m.added_at, m.added_by
  FROM org_members m JOIN groups root ON root.org_id = m.org_id
  JOIN groups sg ON sg.parent_id = root.id
   AND sg.name = CASE m.role WHEN 'admin' THEN 'admins' WHEN 'member' THEN 'members' ELSE 'viewers' END
  WHERE m.removed_at IS NULL;

INSERT OR IGNORE INTO user_role_mappings (person_id, role_id)
  SELECT p.id, r.id FROM people p, roles r
  WHERE p.is_personal_owner = 1 AND r.client_id IS NULL AND r.name = 'fleet-admin';
INSERT OR IGNORE INTO user_role_mappings (person_id, role_id)
  SELECT DISTINCT t.person_id, r.id FROM client_tokens t, roles r
  WHERE t.person_id IS NOT NULL AND t.revoked_at IS NULL AND t.trusted_at IS NOT NULL
    AND r.client_id IS NULL AND r.name = 'fleet-trusted';
-- The assets-admin role mapping is backfilled in Rust
-- (`Store::backfill_assets_admin_roles`, once, when this migration runs): its
-- predicate reads client_tokens.org_id (migration 066), which a database that
-- skipped 064-068 only gets from `repair_skipped_main_migrations`, after
-- the migrations ran.

-- Created after the seed above, not after the DDL: the seed's INSERTs would
-- otherwise bump the epoch of a fresh database (migration 060 expects 0).
-- Every later write to the realm model re-resolves every cached caller. The
-- UPDATE triggers of roles and groups skip `uuid` and the other columns that
-- decide nothing (the precedent: client_tokens skips last_seen_at), so the
-- uuid backfill on open never touches the epoch.
CREATE TRIGGER IF NOT EXISTS auth_epoch_roles_insert
  AFTER INSERT ON roles
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_roles_update
  AFTER UPDATE OF client_id, name ON roles
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_roles_delete
  AFTER DELETE ON roles
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_role_composites_insert
  AFTER INSERT ON role_composites
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_role_composites_update
  AFTER UPDATE ON role_composites
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_role_composites_delete
  AFTER DELETE ON role_composites
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_groups_insert
  AFTER INSERT ON groups
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_groups_update
  AFTER UPDATE OF name, parent_id, managed, org_id ON groups
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_groups_delete
  AFTER DELETE ON groups
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_members_insert
  AFTER INSERT ON group_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_members_update
  AFTER UPDATE ON group_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_members_delete
  AFTER DELETE ON group_members
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_user_role_mappings_insert
  AFTER INSERT ON user_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_user_role_mappings_update
  AFTER UPDATE ON user_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_user_role_mappings_delete
  AFTER DELETE ON user_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_role_mappings_insert
  AFTER INSERT ON group_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_role_mappings_update
  AFTER UPDATE ON group_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_group_role_mappings_delete
  AFTER DELETE ON group_role_mappings
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (178);
