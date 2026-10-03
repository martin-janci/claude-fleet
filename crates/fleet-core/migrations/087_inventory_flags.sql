-- Assets S1a: what a scan can tell about an installed asset the catalog does
-- not know. `secret_like` = its config looks like it carries a credential;
-- `fleet_owned` = fleet provisioned it (its own hooks, MCP entry, skills).
-- `host_hash` is now also filled for these rows. ADD COLUMN is not
-- idempotent: guarded in schema.rs.
ALTER TABLE asset_inventory ADD COLUMN secret_like INTEGER NOT NULL DEFAULT 0;
ALTER TABLE asset_inventory ADD COLUMN fleet_owned INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (87);
