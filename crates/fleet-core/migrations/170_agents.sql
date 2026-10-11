-- Agent runtime (docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md,
-- step 1): the rules a person grants the built-in agents (the PR reviewer,
-- the support agent), what each rule's poller last saw, the jobs the
-- pollers found, and the writes a job proposes that a person answers.
--
-- agent_rules: no row, no agent. A person writes it (`fleet-hub agents
-- grant`, or the `agents` tool on the owner's own device); nothing an agent
-- holding the master token can call writes it.
--   agent        review | support
--   scope        what the rule watches: gh:<owner>/<repo> for review,
--                jira:<tracker id>:<PROJECT> for support. One rule per
--                (agent, scope).
--   level        watch    run, keep the result in fleet, write nothing
--                propose  queue each write for a person (agent_actions)
--                act      write; a kind the agent always asks about is
--                         still queued
--   host_alias   where the agent's runs happen
--   project_id   the project whose checkout a run uses; NULL when none
--   config       the agent's per-rule options, a JSON object
--   expires_at   unix seconds; NULL never expires. An expired rule is
--                treated as absent.
CREATE TABLE IF NOT EXISTS agent_rules (
  id          INTEGER PRIMARY KEY,
  agent       TEXT NOT NULL CHECK (agent IN ('review', 'support')),
  scope       TEXT NOT NULL CHECK (length(scope) BETWEEN 1 AND 300),
  level       TEXT NOT NULL CHECK (level IN ('watch', 'propose', 'act')),
  host_alias  TEXT NOT NULL,
  project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
  config      TEXT NOT NULL DEFAULT '{}'
              CHECK (json_valid(config) AND length(config) <= 8000),
  granted_by  TEXT NOT NULL,
  granted_at  INTEGER NOT NULL,
  expires_at  INTEGER,
  UNIQUE (agent, scope)
);

-- agent_cursors: where a rule's poller stopped, so a restart neither
-- replays nor skips. `cursor` is the poller's own text (a time, a comment
-- id); fleet does not read it.
CREATE TABLE IF NOT EXISTS agent_cursors (
  rule_id    INTEGER PRIMARY KEY REFERENCES agent_rules(id) ON DELETE CASCADE,
  polled_at  INTEGER NOT NULL,
  cursor     TEXT
);

-- agent_jobs: one piece of work a poller found. `dedup_key` makes a job
-- happen once: a second poll that finds the same thing writes nothing.
-- A job outlives its rule (rule_id SET NULL), so its record stays.
--   subject     what the job is about: gh:<owner>/<repo>#<n>, jira:<KEY>
--   trigger     opened | pushed | comment | assigned | manual | reverify
--   state       pending | running | done | failed | skipped | needs_person
--   lease_until a running job's claim; past it the job is pending again
--   result      the run's JSON answer, as the agent keeps it
CREATE TABLE IF NOT EXISTS agent_jobs (
  id               INTEGER PRIMARY KEY,
  rule_id          INTEGER REFERENCES agent_rules(id) ON DELETE SET NULL,
  agent            TEXT NOT NULL CHECK (agent IN ('review', 'support')),
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
CREATE INDEX IF NOT EXISTS agent_jobs_due ON agent_jobs (state, next_attempt_at);
CREATE INDEX IF NOT EXISTS agent_jobs_subject ON agent_jobs (agent, subject, created_at);

-- agent_actions: a write a job proposes (post a review, comment on a
-- ticket, create a bug). `payload` is exactly what will be sent, so a
-- person approves the bytes. Approving hands the action to the agent's
-- executor; the executor records executed or failed.
--   state  pending | approved | rejected | executed | failed | expired
CREATE TABLE IF NOT EXISTS agent_actions (
  id           INTEGER PRIMARY KEY,
  job_id       INTEGER NOT NULL REFERENCES agent_jobs(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,
  payload      TEXT NOT NULL CHECK (json_valid(payload)),
  state        TEXT NOT NULL DEFAULT 'pending' CHECK (state IN
                 ('pending', 'approved', 'rejected', 'executed', 'failed', 'expired')),
  decided_by   TEXT,
  decided_at   INTEGER,
  executed_at  INTEGER,
  error        TEXT,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS agent_actions_state ON agent_actions (state, created_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (170);
