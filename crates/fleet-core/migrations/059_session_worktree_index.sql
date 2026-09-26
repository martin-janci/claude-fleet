-- Hub store latency, task 6: `alive_sessions_for_worktree` (called once per
-- worktree by `list_worktrees`, and inline by `delete_worktree`'s occupant
-- guard) filters `sessions.worktree_id=? AND status='running' AND lost_at IS
-- NULL` with no index on `worktree_id`, so every call is a full table scan.
-- Partial (`WHERE lost_at IS NULL`): a lost/ghosted row can never be an
-- occupant, and live rows are the overwhelming minority once a fleet has run
-- for a while, so the index stays small and cheap to maintain on every
-- ghosting write.
CREATE INDEX IF NOT EXISTS idx_sessions_worktree_live
  ON sessions(worktree_id) WHERE lost_at IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (59);
