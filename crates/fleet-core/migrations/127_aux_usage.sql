-- 127: `aux_usage`, the cost of the `claude -p` runs fleet starts on its
-- own behalf (Orbit Fleet redesign step 8.2,
-- docs/ux/2026-10-08-orbit-fleet-redesign/transition-plan.md). A session's
-- spend is in `sessions.usage_*`; these runs have no session row, so until
-- now they were not costed at all.
--
-- origin   what ran it: 'planner' (a mission's planner), 'summary' (a past
--          conversation's summary). A later origin is a new value, not a
--          new table.
-- mission_id         the mission a planner run planned (the budget brake
--                    sums it with the mission's workers' spend)
-- claude_session_id  the conversation a summary read
-- cost_micros        `total_cost_usd` as `claude` reported it, micro-USD;
--                    0 when it reported none
--
-- New objects only, `IF NOT EXISTS`, safe to re-run.
CREATE TABLE IF NOT EXISTS aux_usage (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  origin            TEXT    NOT NULL,
  host_alias        TEXT    NOT NULL,
  model             TEXT    NOT NULL,
  mission_id        INTEGER REFERENCES orchestration_projects(id) ON DELETE SET NULL,
  org_id            INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  claude_session_id TEXT,
  input_tokens      INTEGER,
  output_tokens     INTEGER,
  cost_micros       INTEGER NOT NULL DEFAULT 0,
  at                INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_aux_usage_mission
  ON aux_usage(mission_id) WHERE mission_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_aux_usage_at ON aux_usage(at);

INSERT OR IGNORE INTO schema_version (version) VALUES (127);
