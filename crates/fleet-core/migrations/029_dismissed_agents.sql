-- Agents (claude agents --json rows outside tmux) the user removed from the
-- list. Reconcile skips an agent while dismissed_at >= its last activity.
CREATE TABLE IF NOT EXISTS dismissed_agents (
  host_alias TEXT NOT NULL,
  claude_session_id TEXT NOT NULL,
  dismissed_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, claude_session_id)
);
INSERT OR IGNORE INTO schema_version (version) VALUES (29);
