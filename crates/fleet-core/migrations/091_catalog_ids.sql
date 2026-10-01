-- Assets S1b (M2): a layer belongs to a catalog; a managed inventory row
-- names the catalog its asset came from. Existing rows belong to
-- `personal`. host_layers is rebuilt (its primary key changes).
--
-- Rebuilt WITHOUT `ALTER TABLE … RENAME` — 071's fix applies here too: a
-- rename re-parses every trigger in the schema, and on a database the
-- migration-collision repair (`repair_skipped_main_migrations`, which runs
-- AFTER all pending migrations including this one) has not reached yet,
-- `sessions_row_version_bump` can already name a column that repair has not
-- added back, and the rename fails validating it. Copy out, drop, recreate,
-- copy back: no rename, same result.
-- Assets M3 (R1): never assume host_layers exists. A database from the
-- 033/035 collision family can reach this migration without it, and
-- `repair_skipped_main_migrations` — which recreates it — only runs after
-- every pending migration. 033's shape; the rebuild below converts it.
CREATE TABLE IF NOT EXISTS host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,
  position   INTEGER NOT NULL DEFAULT 0,
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);

CREATE TABLE IF NOT EXISTS host_layers_new (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,
  position   INTEGER NOT NULL DEFAULT 0,
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, catalog_id, layer_name)
);
-- Only rows whose host still exists: a dangling host_layers row (no
-- matching hosts.alias) can never resolve anyway, and copying it forward
-- would resurface as a fatal "added" FK violation after the rebuild
-- reassigns rowids and FK ids (see final-review I1).
INSERT OR IGNORE INTO host_layers_new (host_alias, catalog_id, layer_name, axis, position, active)
  SELECT hl.host_alias, c.id, hl.layer_name, hl.axis, hl.position, hl.active
  FROM host_layers hl
  JOIN hosts h ON h.alias = hl.host_alias
  JOIN catalogs c ON c.org_id IS NULL;
DROP TABLE host_layers;
CREATE TABLE host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  catalog_id INTEGER NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,
  position   INTEGER NOT NULL DEFAULT 0,
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, catalog_id, layer_name)
);
INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active)
  SELECT host_alias, catalog_id, layer_name, axis, position, active FROM host_layers_new;
DROP TABLE host_layers_new;
CREATE UNIQUE INDEX IF NOT EXISTS idx_host_active_role
  ON host_layers(host_alias, catalog_id) WHERE axis = 'role' AND active = 1;

ALTER TABLE asset_inventory ADD COLUMN catalog_id INTEGER REFERENCES catalogs(id) ON DELETE SET NULL;
UPDATE asset_inventory SET catalog_id = (SELECT id FROM catalogs WHERE org_id IS NULL)
  WHERE state NOT IN ('unmanaged', 'orphan');

INSERT OR IGNORE INTO schema_version (version) VALUES (91);
