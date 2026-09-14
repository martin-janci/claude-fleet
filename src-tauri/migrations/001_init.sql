CREATE TABLE IF NOT EXISTS hosts (
  alias            TEXT PRIMARY KEY,
  last_pinged_at   INTEGER,
  reachable        INTEGER NOT NULL DEFAULT 0,
  claude_version   TEXT,
  tmux_version     TEXT,
  hidden           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS projects (
  id               INTEGER PRIMARY KEY,
  owner            TEXT NOT NULL,
  repo             TEXT NOT NULL,
  base_path        TEXT NOT NULL,
  last_session_at  INTEGER,
  UNIQUE (owner, repo)
);

CREATE TABLE IF NOT EXISTS worktrees (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id),
  name         TEXT NOT NULL,
  path         TEXT NOT NULL,
  branch       TEXT,
  UNIQUE (project_id, name)
);

CREATE TABLE IF NOT EXISTS sessions (
  id                  INTEGER PRIMARY KEY,
  tmux_name           TEXT NOT NULL,
  host_alias          TEXT NOT NULL REFERENCES hosts(alias),
  project_id          INTEGER REFERENCES projects(id),
  worktree_id         INTEGER REFERENCES worktrees(id),
  created_at          INTEGER NOT NULL,
  last_activity_at    INTEGER NOT NULL,
  status              TEXT NOT NULL,
  frozen_scrollback   TEXT,
  notes               TEXT,
  UNIQUE (host_alias, tmux_name)
);

-- The `handoffs` table (Handoff, spec §8.3) was never used and is dropped by
-- migration 022. It is no longer created here: this bootstrap script re-runs
-- on every launch, so keeping the CREATE would resurrect the dropped table.
-- (`sessions.frozen_scrollback` above stays in the CREATE so 022 can drop it
-- unconditionally; the CREATE is a no-op on every existing database.)

CREATE TABLE IF NOT EXISTS settings (
  key    TEXT PRIMARY KEY,
  value  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER PRIMARY KEY
);

INSERT OR IGNORE INTO schema_version (version) VALUES (1);
