-- Host-scoped worktree rows.
--
-- `worktrees` rows used to mirror only the LOCAL checkout's
-- `git worktree list`, keyed UNIQUE(project_id, name), so a remote host's
-- EnterWorktree hook had nowhere to store its worktree without overwriting a
-- same-named local row. Each row now belongs to one host. Every existing row
-- came from the local scan, so it backfills to 'local'.
--
-- SQLite cannot change a table's UNIQUE constraint in place, so this is its
-- documented table rebuild: create, copy (ids preserved, so
-- sessions.worktree_id stays valid), drop, rename. `Store::migrate` runs
-- pending migrations with foreign keys OFF and requires an empty
-- `PRAGMA foreign_key_check` before each commit, as that procedure needs.
-- No join index to recreate: `project_id` joins use the new
-- UNIQUE(project_id, host_alias, name) index (see 007).
CREATE TABLE worktrees_new (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id),
  host_alias   TEXT NOT NULL DEFAULT 'local',
  name         TEXT NOT NULL,
  path         TEXT NOT NULL,
  branch       TEXT,
  UNIQUE (project_id, host_alias, name)
);

INSERT INTO worktrees_new (id, project_id, host_alias, name, path, branch)
  SELECT id, project_id, 'local', name, path, branch FROM worktrees;

DROP TABLE worktrees;

ALTER TABLE worktrees_new RENAME TO worktrees;

-- Per-host lookups: cwd linking (`HostPaths`) and the ExitWorktree delete.
CREATE INDEX IF NOT EXISTS idx_worktrees_host_path ON worktrees(host_alias, path);

INSERT OR IGNORE INTO schema_version (version) VALUES (24);
