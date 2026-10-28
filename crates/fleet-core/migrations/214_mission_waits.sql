-- Mission improvements M3 (docs/superpowers/specs/2026-10-26-mission-
-- improvements-design.md §3): why a mission's ready work waits.
--
-- mission_waits  one row per mission and cause (`person`, `grant`,
--                `limit`, `brake`, `slot`, `lane`, `touches`): `secs` the
--                time some ready work of the mission waited on it, `open`
--                1 while it does now. The loop's tick, and a change of the
--                mission's state, add the time since the last look to the
--                open causes and open the ones it sees now.
CREATE TABLE IF NOT EXISTS mission_waits (
  orchestration_project_id INTEGER NOT NULL
    REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  cause TEXT NOT NULL,
  secs  INTEGER NOT NULL DEFAULT 0,
  open  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (orchestration_project_id, cause)
);

-- orchestration_projects
--   waits_at  when the causes were last looked at; the next look adds the
--             time since to the causes open then.
ALTER TABLE orchestration_projects ADD COLUMN waits_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (214);
