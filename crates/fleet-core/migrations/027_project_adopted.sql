-- Marks a project row registered by `service::add_project`'s `folder`
-- source (adopting an existing checkout already on disk, possibly OUTSIDE
-- the local projects root) so `refresh_projects`'s stale-rows sweep never
-- deletes it: an adopted row living outside the scanned root and unreferenced
-- by any session is its normal shape, not evidence of staleness. 0/1 as
-- INTEGER (SQLite has no native boolean); every other row defaults to 0.
ALTER TABLE projects ADD COLUMN adopted INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (27);
