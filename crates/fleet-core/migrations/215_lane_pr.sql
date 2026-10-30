-- Mission improvements M4 (docs/superpowers/specs/2026-10-26-mission-
-- improvements-design.md §7): a lane's draft pull request, opened by fleet
-- under `policy.lane_pr = draft`.
--
-- mission_lanes
--   pr_url   the lane branch's pull request, once fleet opened or found it
--   pr_head  the commit fleet last pushed to the lane's branch: the newest
--            verified task's, never the lead's work in progress
ALTER TABLE mission_lanes ADD COLUMN pr_url TEXT;
ALTER TABLE mission_lanes ADD COLUMN pr_head TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (215);
