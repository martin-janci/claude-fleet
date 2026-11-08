-- A dashboard's version before its last replacement, so the person can
-- take back a change Control made at their request (`dashboards { undo }`,
-- `service::dashboards`). One step only: an undo clears it.
--
-- pinned_dashboards
--   previous_spec    the spec the last replacement overwrote; NULL = none
--   previous_device  the device that pinned that version
ALTER TABLE pinned_dashboards ADD COLUMN previous_spec TEXT;
ALTER TABLE pinned_dashboards ADD COLUMN previous_device TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (220);
