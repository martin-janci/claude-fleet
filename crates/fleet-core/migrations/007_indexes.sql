-- Composite index for `list_related_sessions`, which filters
-- `WHERE project_id=? AND worktree_key=?` — the only hot query not already
-- served by an existing index. `WHERE host_alias=?` / `host_alias=?+tmux_name`
-- lookups use the UNIQUE(host_alias, tmux_name) index; `worktrees.project_id`
-- joins use UNIQUE(project_id, name); id lookups use the primary key.
CREATE INDEX IF NOT EXISTS idx_sessions_project_wtkey
  ON sessions(project_id, worktree_key);

INSERT OR IGNORE INTO schema_version (version) VALUES (7);
