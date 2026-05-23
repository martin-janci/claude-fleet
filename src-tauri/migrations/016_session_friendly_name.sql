-- A short, human-readable label the in-session Claude agent sets via MCP
-- (set_friendly_name) when it picks up a task. Display-only — tmux_name
-- remains the row's identity. NULL until the agent populates it.
ALTER TABLE sessions ADD COLUMN friendly_name TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (16);
