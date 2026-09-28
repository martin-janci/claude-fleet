-- 075: `sessions.launch_model`, the `claude --model` a Claude session runs
-- under: set by `new_session { model }` and by a `/model <x>` sent through
-- fleet, and passed again by recreate / restart / repair so a rebuilt pane
-- keeps it. NULL = the host's default. The effort half reuses
-- `sessions.effort_level` (010), which nothing else writes. Not a
-- `SessionRow` field: the row-version trigger does not watch it. One ADD
-- COLUMN, guarded in schema.rs.
ALTER TABLE sessions ADD COLUMN launch_model TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (75);
