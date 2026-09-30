-- Assets S1b (M1): a catalog is a source with an owner. `org_id NULL` is the
-- personal catalog, and only it (the CHECK). Replaces the single-row
-- `catalog_config`, which stays but is no longer read.
CREATE TABLE IF NOT EXISTS catalogs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  name           TEXT    NOT NULL UNIQUE,
  repo_path      TEXT    NOT NULL,
  remote_url     TEXT,
  org_id         INTEGER REFERENCES orgs(id) ON DELETE RESTRICT,
  head_commit    TEXT,
  last_loaded_at INTEGER,
  created_at     INTEGER NOT NULL,
  CHECK ((name = 'personal') = (org_id IS NULL))
);

INSERT OR IGNORE INTO catalogs (name, repo_path, remote_url, org_id, head_commit, last_loaded_at, created_at)
  SELECT 'personal', repo_path, remote_url, NULL, head_commit, last_loaded_at, CAST(strftime('%s','now') AS INTEGER)
  FROM catalog_config WHERE id = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (89);
