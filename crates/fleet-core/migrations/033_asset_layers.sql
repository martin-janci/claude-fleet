-- Per-host layer assignment (asset catalog layers). Layer DEFINITIONS live in
-- the catalog repo under layers/*.yaml; only the assignment is fleet state.
-- A host with no row here resolves to the whole catalog, which is the
-- pre-layers behaviour.
CREATE TABLE IF NOT EXISTS host_layers (
  host_alias TEXT    NOT NULL REFERENCES hosts(alias),
  layer_name TEXT    NOT NULL,
  axis       TEXT    NOT NULL,             -- 'role' | 'context'
  position   INTEGER NOT NULL DEFAULT 0,   -- context application order
  active     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (host_alias, layer_name)
);

-- At most one active role per host, enforced by the schema rather than by
-- application code.
CREATE UNIQUE INDEX IF NOT EXISTS idx_host_active_role
  ON host_layers(host_alias) WHERE axis = 'role' AND active = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (33);
