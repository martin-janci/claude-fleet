-- Configurable agents, step 2 (docs/superpowers/specs/
-- 2026-10-12-configurable-agents-design.md, *D. Stores*): a rule may name a
-- catalog `fleet_agent` definition instead of a built-in agent, and binds
-- the definition's store roles; the fleet store is a new table.
--
-- agent_rules gains:
--   agent        'fleet' besides review | support
--   definition   the catalog fleet_agent a 'fleet' rule runs; NULL otherwise
--   bindings     a JSON array: one binding per store role, an MCP server
--                with its env overrides (secret NAMES, never values) or a
--                fleet namespace. `service::fleet_agents::bindings` checks it.
-- A 'fleet' rule's scope is its instance name, so two instances of one
-- definition are two rules.
-- agent_jobs: `agent` admits 'fleet'.
--
-- Both CHECKs live in the CREATE, which SQLite cannot change in place, so
-- the two tables are rebuilt the way 142 rebuilds `session_grants`: copy
-- aside, drop, create under the same name, copy back with ids preserved,
-- recreate the indexes as 173 wrote them. `Store::migrate` runs this with
-- foreign keys OFF (agent_cursors, agent_jobs, agent_actions and
-- review_records reference these tables by name, and keep doing so) and
-- requires an empty `PRAGMA foreign_key_check` before the commit; the
-- `already_applied` guard (`agent_rules_has_definition`) keeps a re-run from
-- rebuilding again.
CREATE TEMP TABLE agent_rules_before_185 AS SELECT * FROM agent_rules;
CREATE TEMP TABLE agent_jobs_before_185 AS SELECT * FROM agent_jobs;

DROP TABLE agent_rules;
DROP TABLE agent_jobs;

CREATE TABLE agent_rules (
  id          INTEGER PRIMARY KEY,
  agent       TEXT NOT NULL CHECK (agent IN ('review', 'support', 'fleet')),
  scope       TEXT NOT NULL CHECK (length(scope) BETWEEN 1 AND 300),
  level       TEXT NOT NULL CHECK (level IN ('watch', 'propose', 'act')),
  host_alias  TEXT NOT NULL,
  project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
  config      TEXT NOT NULL DEFAULT '{}'
              CHECK (json_valid(config) AND length(config) <= 8000),
  granted_by  TEXT NOT NULL,
  granted_at  INTEGER NOT NULL,
  expires_at  INTEGER,
  definition  TEXT CHECK (definition IS NULL OR length(definition) BETWEEN 1 AND 100),
  bindings    TEXT NOT NULL DEFAULT '[]'
              CHECK (json_valid(bindings) AND length(bindings) <= 8000),
  CHECK ((agent = 'fleet') = (definition IS NOT NULL)),
  UNIQUE (agent, scope)
);

INSERT INTO agent_rules
  (id, agent, scope, level, host_alias, project_id, config, granted_by, granted_at, expires_at)
  SELECT id, agent, scope, level, host_alias, project_id, config, granted_by, granted_at, expires_at
    FROM agent_rules_before_185;

CREATE TABLE agent_jobs (
  id               INTEGER PRIMARY KEY,
  rule_id          INTEGER REFERENCES agent_rules(id) ON DELETE SET NULL,
  agent            TEXT NOT NULL CHECK (agent IN ('review', 'support', 'fleet')),
  subject          TEXT NOT NULL,
  trigger          TEXT NOT NULL,
  dedup_key        TEXT NOT NULL,
  state            TEXT NOT NULL DEFAULT 'pending' CHECK (state IN
                     ('pending', 'running', 'done', 'failed', 'skipped', 'needs_person')),
  attempts         INTEGER NOT NULL DEFAULT 0,
  next_attempt_at  INTEGER NOT NULL,
  lease_until      INTEGER,
  error_kind       TEXT,
  error            TEXT,
  result           TEXT CHECK (result IS NULL OR json_valid(result)),
  cost_micros      INTEGER NOT NULL DEFAULT 0,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  UNIQUE (agent, dedup_key)
);

INSERT INTO agent_jobs
  (id, rule_id, agent, subject, trigger, dedup_key, state, attempts, next_attempt_at,
   lease_until, error_kind, error, result, cost_micros, created_at, updated_at)
  SELECT id, rule_id, agent, subject, trigger, dedup_key, state, attempts, next_attempt_at,
         lease_until, error_kind, error, result, cost_micros, created_at, updated_at
    FROM agent_jobs_before_185;

DROP TABLE agent_rules_before_185;
DROP TABLE agent_jobs_before_185;

CREATE INDEX IF NOT EXISTS agent_jobs_due ON agent_jobs (state, next_attempt_at);
CREATE INDEX IF NOT EXISTS agent_jobs_subject ON agent_jobs (agent, subject, created_at);
CREATE INDEX IF NOT EXISTS agent_jobs_created ON agent_jobs (created_at);

-- agent_records: the fleet store. A rule reads the namespaces it binds
-- (`rule:<id>`, private, or `shared:<name>`), before a run through
-- `run.context` and during one through the `fleet-records` server; it
-- writes them only through `record` actions.
--   kind        what the record is; `record` unless an action says
--   value       JSON, at most 64 KiB
--   expires_at  unix seconds; NULL keeps it. An expired record reads as
--               absent and is swept.
--   written_by_job  the job whose action wrote it (SET NULL when it goes)
CREATE TABLE IF NOT EXISTS agent_records (
  namespace       TEXT NOT NULL CHECK (length(namespace) BETWEEN 1 AND 120),
  kind            TEXT NOT NULL DEFAULT 'record' CHECK (length(kind) BETWEEN 1 AND 40),
  key             TEXT NOT NULL CHECK (length(key) BETWEEN 1 AND 300),
  value           TEXT NOT NULL CHECK (json_valid(value) AND length(value) <= 65536),
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  expires_at      INTEGER,
  written_by_job  INTEGER REFERENCES agent_jobs(id) ON DELETE SET NULL,
  PRIMARY KEY (namespace, kind, key)
);
CREATE INDEX IF NOT EXISTS agent_records_recent ON agent_records (namespace, updated_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (185);
