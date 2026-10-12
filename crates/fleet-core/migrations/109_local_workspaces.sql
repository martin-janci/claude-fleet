-- Local workspace sync, Phase 1
-- (docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md): one
-- remote worktree bound to one directory on the desktop's machine, kept in
-- step both ways. Keyed by the worktree's identity — host, owner/repo TEXT
-- (project ids are re-derived, review C22) and worktree key — never by a
-- session: sessions come and go on one worktree.
CREATE TABLE IF NOT EXISTS local_workspaces (
  id             INTEGER PRIMARY KEY,
  host_alias     TEXT    NOT NULL,
  owner          TEXT    NOT NULL,
  repo           TEXT    NOT NULL,
  worktree_key   TEXT    NOT NULL,
  remote_path    TEXT    NOT NULL,
  local_path     TEXT    NOT NULL UNIQUE,
  session_id     INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
  paused         INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1)),
  excludes       TEXT    NOT NULL DEFAULT '[]',
  state          TEXT    NOT NULL DEFAULT 'syncing',
  last_sync_at   INTEGER,
  last_error     TEXT,
  pending_local  INTEGER NOT NULL DEFAULT 0,
  pending_remote INTEGER NOT NULL DEFAULT 0,
  skipped        INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL,
  UNIQUE (host_alias, owner, repo, worktree_key)
);

-- The BASE: per path, the content both sides last agreed on and each side's
-- (size, mtime) when it was recorded. A NULL mtime means "re-check next
-- pass" (recorded too close to the file's own mtime to trust the stat).
CREATE TABLE IF NOT EXISTS local_workspace_files (
  workspace_id  INTEGER NOT NULL REFERENCES local_workspaces(id) ON DELETE CASCADE,
  path          TEXT    NOT NULL,
  sha256        TEXT    NOT NULL,
  local_size    INTEGER,
  local_mtime   INTEGER,
  remote_size   INTEGER,
  remote_mtime  INTEGER,
  PRIMARY KEY (workspace_id, path)
) WITHOUT ROWID;

-- Open conflicts: a path both sides changed differently. Nothing is written
-- to either side for it until it is resolved. Each side's content hash and
-- stat as last seen (NULL = absent on that side), so a later pass can tell
-- whether either side moved since; `resolution` is the person's pick, which
-- the next pass carries out under the same guards as any other write.
CREATE TABLE IF NOT EXISTS local_workspace_conflicts (
  workspace_id  INTEGER NOT NULL REFERENCES local_workspaces(id) ON DELETE CASCADE,
  path          TEXT    NOT NULL,
  kind          TEXT    NOT NULL CHECK (kind IN
                  ('both_modified', 'both_added', 'local_deleted', 'remote_deleted')),
  detected_at   INTEGER NOT NULL,
  local_sha     TEXT,
  local_size    INTEGER,
  local_mtime   INTEGER,
  remote_sha    TEXT,
  remote_size   INTEGER,
  remote_mtime  INTEGER,
  resolution    TEXT    CHECK (resolution IS NULL OR resolution IN ('local', 'remote')),
  PRIMARY KEY (workspace_id, path)
) WITHOUT ROWID;

INSERT OR IGNORE INTO schema_version (version) VALUES (109);
