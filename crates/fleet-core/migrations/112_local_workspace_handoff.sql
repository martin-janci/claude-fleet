-- Local workspace, Phases 2 and 3
-- (docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md).
--
-- Who drives the worktree: `shared` (both edit, sync resolves), `developer`
-- (the agent was told to keep its hands off) or `agent` (handed back).
-- `driver_since` is when that was set.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs on `driver_since`.
ALTER TABLE local_workspaces ADD COLUMN driver TEXT NOT NULL DEFAULT 'shared'
  CHECK (driver IN ('shared', 'developer', 'agent'));
ALTER TABLE local_workspaces ADD COLUMN driver_since INTEGER;

-- What each side changed and nobody has looked at yet: one row per path,
-- written when a pass carries a change across. `origin` is `local` (pushed
-- to the host: the developer's edit) or `remote` (pulled here: the agent's,
-- or anyone's on the host). The latest carry wins. A row goes away when the
-- change is handed to the agent, committed, discarded or dismissed.
CREATE TABLE IF NOT EXISTS local_workspace_activity (
  workspace_id  INTEGER NOT NULL REFERENCES local_workspaces(id) ON DELETE CASCADE,
  path          TEXT    NOT NULL,
  origin        TEXT    NOT NULL CHECK (origin IN ('local', 'remote')),
  change        TEXT    NOT NULL CHECK (change IN ('added', 'modified', 'deleted')),
  at            INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, path)
) WITHOUT ROWID;

INSERT OR IGNORE INTO schema_version (version) VALUES (112);
