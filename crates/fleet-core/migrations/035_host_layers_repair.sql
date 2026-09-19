-- Repair for the 033 collision. `main` shipped `033_asset_layers.sql`; the
-- host-agent branch had independently numbered its own migration 033, and it
-- was renumbered to 034 when the two merged. A database created by the
-- PRE-MERGE branch therefore recorded version 33 for a migration that only
-- added `hosts.transport` — so on the merged head `migrate()` (which runs
-- every migration with `version > MAX(schema_version)`) never offers 033, and
-- `host_layers` is silently never created.
--
-- Re-running the layer DDL here closes that gap. It is the same statements as
-- 033, `IF NOT EXISTS` throughout, so it is a no-op on a fresh database and on
-- any database that came from a released build.
CREATE TABLE IF NOT EXISTS host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,             -- 'role' | 'context'
  position   INTEGER NOT NULL DEFAULT 0,   -- context application order
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_host_active_role
  ON host_layers(host_alias) WHERE axis = 'role' AND active = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (35);
