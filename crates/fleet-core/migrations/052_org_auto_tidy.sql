-- Work graph M7 on M5 (docs/superpowers/plans/2026-09-24-work-graph-m7-self-cleaning-lifecycle.md):
-- orgs.auto_tidy   an org's auto-tidy override: NULL inherits `work.auto_tidy`,
--                  0 off, 1 on. Its own migration (and guard), apart from 051,
--                  so a database that has 051's columns but an `orgs` table
--                  made later still gets it.
ALTER TABLE orgs ADD COLUMN auto_tidy INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (52);
