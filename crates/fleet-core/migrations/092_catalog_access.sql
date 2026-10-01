-- Assets S1b (M3): which org catalogs a host with no org admits, and which
-- catalogs a paired client may manage. A personal grant is a row here plus
-- its mirror `client_tokens.assets_admin_at` (migration 074), which stays
-- for `client list`'s ASSETS column and for the window before a personal
-- catalog exists; nothing else reads it once a personal catalog exists.
-- CREATE IF NOT EXISTS + INSERT OR IGNORE: safe to re-run.
CREATE TABLE IF NOT EXISTS host_catalogs (
  host_alias  TEXT    NOT NULL REFERENCES hosts(alias) ON DELETE CASCADE,
  catalog_id  INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  admitted_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, catalog_id)
);

CREATE TABLE IF NOT EXISTS client_catalog_grants (
  client_id  INTEGER NOT NULL REFERENCES client_tokens(id) ON DELETE CASCADE,
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (client_id, catalog_id)
);

-- Spec, Migration step 3: every client with `assets_admin_at` gets a grant on
-- `personal`. With no personal catalog yet there is nothing to attach it to;
-- `Store::set_catalog_config` runs the same statement when it creates one.
INSERT OR IGNORE INTO client_catalog_grants (client_id, catalog_id, granted_at)
  SELECT t.id, c.id, t.assets_admin_at
  FROM client_tokens t JOIN catalogs c ON c.org_id IS NULL
  WHERE t.assets_admin_at IS NOT NULL;

-- Migration 060's rule — every change to who may do what bumps the epoch —
-- held for the new table too, although `catalog_admin` reads grants live.
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_catalog_grants_insert
  AFTER INSERT ON client_catalog_grants
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_catalog_grants_delete
  AFTER DELETE ON client_catalog_grants
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (92);
