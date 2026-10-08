-- Orchestration O4–O6 (docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md
-- §5, §7): the loop's cards and a person's grants.
--
-- `orchestration_cards` is the confirm queue: each row is one thing the
-- planner (O5) asks to do: run, retry, create, ask, complete, …. A person
-- applies or dismisses it from the desktop or the phone; under a grant
-- (O6) the loop applies what the grant covers itself. `decision_id` makes
-- one planner command one card. The deterministic loop's own steps (§5.3)
-- are derived on every read and never stored here.
--
-- `orchestration_grants` is what a person signed: the level the loop may
-- act at, for one plan_version, until it expires or is revoked; the hosts
-- it may run on (NULL: any the owner may drive), a spend cap in micro-USD
-- and a parallelism cap. Only the newest live grant counts.
CREATE TABLE IF NOT EXISTS orchestration_cards (
  id                       INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL
    REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  decision_id              TEXT    NOT NULL,
  source                   TEXT    NOT NULL,
  kind                     TEXT    NOT NULL,
  work_item_id             INTEGER REFERENCES work_items(id) ON DELETE CASCADE,
  payload                  TEXT,
  state                    TEXT    NOT NULL,
  note                     TEXT,
  created_at               INTEGER NOT NULL,
  decided_at               INTEGER,
  decided_by               TEXT,
  UNIQUE (orchestration_project_id, decision_id)
);
CREATE INDEX IF NOT EXISTS idx_orchestration_cards_open
  ON orchestration_cards(orchestration_project_id, state, id);

CREATE TABLE IF NOT EXISTS orchestration_grants (
  id                       INTEGER PRIMARY KEY AUTOINCREMENT,
  orchestration_project_id INTEGER NOT NULL
    REFERENCES orchestration_projects(id) ON DELETE CASCADE,
  plan_version             INTEGER NOT NULL,
  level                    INTEGER NOT NULL,
  granted_by               TEXT    NOT NULL,
  hosts                    TEXT,
  budget_micros            INTEGER,
  max_parallel             INTEGER,
  created_at               INTEGER NOT NULL,
  expires_at               INTEGER NOT NULL,
  revoked_at               INTEGER
);
CREATE INDEX IF NOT EXISTS idx_orchestration_grants_live
  ON orchestration_grants(orchestration_project_id, id DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (118);
