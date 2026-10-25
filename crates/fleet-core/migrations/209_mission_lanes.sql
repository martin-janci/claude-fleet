-- Mission lanes (docs/superpowers/specs/2026-10-23-mission-lanes-design.md
-- §2.2, L2): an ordered set of a mission's tasks in one repository that one
-- lead session will work through on one branch (L3). L2 records them only;
-- nothing runs differently.
--
-- mission_lanes
--   name             what the plan or the planner called it ("backend"),
--                    one per mission
--   project_id       its repository; NULL until one is known (a plan's lane
--                    with no repo of its own)
--   branch, worktree_id, lead_session_id, relay
--                    the lead session's checkout and how many leads it has
--                    had (L3); NULL / 0 in L2
--   state            idle | held | done: only what a person or the loop
--                    sets is stored; working / waiting are derived on read
--
-- work_items
--   lane_id          the lane a member task is in; NULL for a task in no
--                    lane (and for every task outside a mission)
CREATE TABLE IF NOT EXISTS mission_lanes (
  id                       INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL
                           REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  name                     TEXT    NOT NULL,
  project_id               INTEGER REFERENCES projects(id) ON DELETE SET NULL,
  branch                   TEXT,
  worktree_id              INTEGER REFERENCES worktrees(id) ON DELETE SET NULL,
  lead_session_id          INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
  relay                    INTEGER NOT NULL DEFAULT 0,
  state                    TEXT    NOT NULL DEFAULT 'idle',
  created_at               INTEGER NOT NULL,
  updated_at               INTEGER NOT NULL,
  UNIQUE (orchestration_project_id, name)
);

-- ADD COLUMN is not idempotent: the migration is guarded on it.
ALTER TABLE work_items ADD COLUMN lane_id INTEGER
  REFERENCES mission_lanes(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_lane ON work_items(lane_id)
  WHERE lane_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (209);
