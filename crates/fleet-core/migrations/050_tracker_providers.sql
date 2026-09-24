-- Work graph M6 (docs/superpowers/plans/2026-09-24-work-graph-m6-more-providers.md):
-- what more tracker providers need from the schema.
--
-- tracker_views.sync_mark  an opaque per-view mark for a provider whose
--                          incremental reads are a sync token (Asana's events
--                          API), not a time watermark. NULL: none yet (the
--                          next pass lists the view whole and asks for one).
-- trackers.settings        what the ADMIN set, as JSON — kept apart from
--                          `config`, which every probe replaces: GitHub's
--                          repos, Asana's section → status map, Jira Data
--                          Center's extra CA and private-network opt-in.
--                          Never a secret (those stay in tracker_secrets).
ALTER TABLE tracker_views ADD COLUMN sync_mark TEXT;
ALTER TABLE trackers ADD COLUMN settings TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (50);
