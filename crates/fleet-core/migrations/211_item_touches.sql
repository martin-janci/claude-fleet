-- Lanes L5 (docs/superpowers/specs/2026-10-23-mission-lanes-design.md
-- §5.4): the paths a task will change, so the loop keeps work that would
-- collide from running side by side.
--
-- work_items
--   touches_json  a JSON array of repository-relative paths or globs
--                 ("crates/fleet-core/src/store/**"), set by the planner's
--                 create_item; NULL when nobody said. Read apart from
--                 WorkItemRow (`Store::item_touches`).
ALTER TABLE work_items ADD COLUMN touches_json TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (211);
