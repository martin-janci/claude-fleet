-- Asset catalog (sub-project 1): where the catalog repo lives, and the
-- per-host / per-harness drift state of every catalog asset as of the last
-- scan. See docs/superpowers/specs/2026-09-14-asset-catalog-design.md.
CREATE TABLE IF NOT EXISTS catalog_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  repo_path TEXT NOT NULL,
  remote_url TEXT,
  head_commit TEXT,
  last_loaded_at INTEGER
);

CREATE TABLE IF NOT EXISTS asset_inventory (
  host_alias TEXT NOT NULL,
  harness TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  state TEXT NOT NULL,
  catalog_hash TEXT,
  host_hash TEXT,
  scanned_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, harness, kind, name)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (30);
