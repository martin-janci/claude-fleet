-- Assets M5 (Rulings R4): on a `drifted` managed inventory row, which side
-- moved. 'host' = the copy on the host is no longer what fleet wrote (a
-- person edited it); 'catalog' = the host copy is exactly what fleet wrote
-- and the catalog moved on, so a sync updates it safely; NULL = not
-- drifted, not managed, or the host's manifest entry predates the file
-- hashes that tell the two apart. ADD COLUMN is not idempotent: guarded in
-- schema.rs.
ALTER TABLE asset_inventory ADD COLUMN drift_side TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (96);
