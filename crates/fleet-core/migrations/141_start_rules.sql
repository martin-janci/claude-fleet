-- Start rules (Orbit Fleet redesign step 8.11, "From AI to rule"): a task
-- key pattern that names the project (and optionally the host) a start of a
-- matching task lands in. An active rule decides before the key's history
-- and before Jev (`service::start_rules`), so no model is asked. Fleet
-- counts a person's identical starts of one key prefix into one project
-- and, after five in a row, offers the rule; a person adds or dismisses it.
--
-- start_rules
--   org_id            the org of the tasks it decides for; NULL = tasks of
--                     no org. Goes with its org.
--   owner_person_id   who added it (NULL while fleet is only counting or
--                     offering, and on a single-person fleet)
--   pattern           a key pattern, `*` for any run of characters
--                     (`PD-*`), matched without regard to case
--   project_id        where a matching start lands; goes with its project
--   host_alias        where it runs; NULL = the project's last host
--   state             counting | offered | active | dismissed. Only an
--                     active rule decides; counting rows are fleet's tally
--                     and a dismissed one is never offered again
--   confirmations     identical person starts in a row (the tally)
--   hits              starts the rule decided
CREATE TABLE IF NOT EXISTS start_rules (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id            INTEGER REFERENCES orgs(id) ON DELETE CASCADE,
  owner_person_id   INTEGER,
  pattern           TEXT    NOT NULL,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  host_alias        TEXT,
  state             TEXT    NOT NULL DEFAULT 'counting'
                    CHECK (state IN ('counting', 'offered', 'active', 'dismissed')),
  confirmations     INTEGER NOT NULL DEFAULT 0,
  hits              INTEGER NOT NULL DEFAULT 0,
  last_hit_at       INTEGER,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);

-- One row per org, pattern and project: the tally and the rule it becomes.
CREATE UNIQUE INDEX IF NOT EXISTS start_rules_one
  ON start_rules (COALESCE(org_id, 0), UPPER(pattern), project_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (141);
