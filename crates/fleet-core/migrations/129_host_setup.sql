-- Orbit Fleet 4.9, the add-host wizard and the fleet-agent install job.
--
-- host_setups: a wizard someone left half-way, one row per SSH alias, so it
-- resumes after the app restarts. `step` is the wizard step (1..5),
-- `checks` the last live-check results (JSON list of SetupCheck), `answers`
-- what the person picked (JSON object, the frontend's own). Deleted when the
-- host is added or the person discards it.
CREATE TABLE IF NOT EXISTS host_setups (
  ssh_alias   TEXT    PRIMARY KEY,
  alias       TEXT    NOT NULL,
  step        INTEGER NOT NULL DEFAULT 1,
  checks      TEXT    NOT NULL DEFAULT '[]',
  answers     TEXT    NOT NULL DEFAULT '{}',
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- agent_installs: one row per fleet-agent install job a hub ran over SSH.
-- state  running | done | failed
-- step   target | download | start | connect | done (where it is, or failed)
-- detail the last line a person reads; never the host token
CREATE TABLE IF NOT EXISTS agent_installs (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  host_alias   TEXT    NOT NULL,
  version      TEXT    NOT NULL,
  state        TEXT    NOT NULL,
  step         TEXT    NOT NULL,
  detail       TEXT,
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER
);

CREATE INDEX IF NOT EXISTS agent_installs_by_host
  ON agent_installs (host_alias, started_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (129);
