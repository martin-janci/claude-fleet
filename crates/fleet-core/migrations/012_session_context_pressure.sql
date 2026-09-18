-- Context-pressure and stuck-state columns populated by the reconcile
-- pane-tail probe (see service::pane_intel). `current_activity` and
-- `claude_status` already exist from 010_claude_agent_fields.sql.
ALTER TABLE sessions ADD COLUMN context_pct REAL;
ALTER TABLE sessions ADD COLUMN stuck_kind TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (12);
