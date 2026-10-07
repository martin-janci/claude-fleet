-- Orchestration O1 (docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md
-- §4.1, §4.2, §4.6): the mission container. "Mission" in the UI,
-- `orchestration_projects` here, because "project" already means a repo.
--
-- `state` is the stored lifecycle a person sees: draft | active | paused |
-- completed | failed | cancelled. The loop's phase (planning, running,
-- waiting, …) is derived on read and never stored (§4.5).
--
-- `root_item_id` is the mission's root work item (an epic or a tracker
-- ticket), so the mission gets that item's TaskDetail, sessions, sprint and
-- thread for free. `level` is the autonomy the owner asks for (L0–L3);
-- what applies is only what a grant covers (O6). `next_wake_at`,
-- `lease_until` and `last_snapshot_at` belong to the loop (O4/O5) and are
-- written by nothing yet. `version` is the expected_version guard, as on
-- work_buckets.
CREATE TABLE IF NOT EXISTS orchestration_projects (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id           INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  owner_person_id  INTEGER,
  root_item_id     INTEGER REFERENCES work_items(id) ON DELETE SET NULL,
  name             TEXT    NOT NULL,
  goal             TEXT    NOT NULL,
  non_goals        TEXT,
  done_when        TEXT,
  mode             TEXT    NOT NULL,
  state            TEXT    NOT NULL,
  level            INTEGER NOT NULL DEFAULT 0,
  policy_json      TEXT,
  plan_version     INTEGER NOT NULL DEFAULT 1,
  next_wake_at     INTEGER,
  lease_until      INTEGER,
  last_snapshot_at INTEGER,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  started_at       INTEGER,
  finished_at      INTEGER,
  version          INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_orch_projects_owner
  ON orchestration_projects(owner_person_id);

-- The repos a mission may run in: an allow-list, not a placement. A run
-- outside it is refused (O4), and a grant takes it as its project_ids (O6).
CREATE TABLE IF NOT EXISTS orchestration_project_repos (
  orchestration_project_id INTEGER NOT NULL
    REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role       TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (orchestration_project_id, project_id)
);

-- The decision and audit log. Not the source of truth: the timeline reads
-- it, and `refused` rows say why the machine did NOT do something.
-- `decision_id` makes one planner command execute once. Capped per mission
-- in `store::orchestration`, older rows folding into one `digest` row.
CREATE TABLE IF NOT EXISTS orchestration_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL
    REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  at           INTEGER NOT NULL,
  kind         TEXT    NOT NULL,
  actor        TEXT    NOT NULL,
  work_item_id INTEGER,
  task_id      INTEGER,
  decision_id  TEXT,
  payload      TEXT
);
CREATE INDEX IF NOT EXISTS idx_orch_events
  ON orchestration_events(orchestration_project_id, at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS ux_orch_events_decision
  ON orchestration_events(decision_id) WHERE decision_id IS NOT NULL;

-- Membership is a flat column rather than derived from the tree, so a
-- follow-up created anywhere belongs to the mission without moving parents.
-- ADD COLUMN is not idempotent: the migration is guarded on it.
ALTER TABLE work_items ADD COLUMN orchestration_project_id INTEGER
  REFERENCES orchestration_projects(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_orch
  ON work_items(orchestration_project_id) WHERE orchestration_project_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (112);
