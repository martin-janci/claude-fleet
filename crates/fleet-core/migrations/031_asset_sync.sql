-- Asset catalog sub-project 2 (sync engine). `managed` marks inventory rows
-- that the host's fleet manifest names. Secrets resolve ${NAME} placeholders
-- at apply time (global value, optional per-host override). sync_runs keeps
-- the last apply summary across restarts.
-- See docs/superpowers/specs/2026-09-14-asset-sync-design.md.
ALTER TABLE asset_inventory ADD COLUMN managed INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS catalog_secrets (
  name TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS catalog_secrets_host (
  host_alias TEXT NOT NULL,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, name)
);

CREATE TABLE IF NOT EXISTS sync_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  finished_at INTEGER NOT NULL,
  summary_json TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (31);
