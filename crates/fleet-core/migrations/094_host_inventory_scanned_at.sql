-- When a host's asset inventory was last REPLACED, per host.
--
-- Until now "when was this host last scanned" was derived as
-- `MAX(scanned_at)` over that host's `asset_inventory` rows, which cannot
-- distinguish a host whose scan SUCCEEDED and found nothing from one that has
-- never been scanned at all. Both read as NULL, so `hosts_due` called the
-- empty host due on every tick and the scan tick swept it over SSH for ever —
-- a fresh host with no assets installed being the ordinary case.
--
-- Written by `replace_host_inventory` whatever the row count, so it records
-- the SCAN and not its findings. Reads fall back to `MAX(scanned_at)` for a
-- database whose last scan predates this column, so no host is re-swept just
-- for upgrading. Goes with the host row on `delete_host`.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN inventory_scanned_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (94);
