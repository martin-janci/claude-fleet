-- New worktree setup (docs/work-graph.md#new-worktree-setup): what a fresh
-- worktree of a project gets before its agent starts.
--
-- projects
--   setup_copy     newline-separated relative globs (`.env`, `config/*.local`)
--                  copied from the project's base checkout into each new
--                  worktree; NULL copies nothing.
--   setup_command  one shell command run in the new worktree after the copy
--                  (`pnpm install`); NULL runs none. A failure stops the
--                  start with "Setup failed" and leaves the worktree.
--                  Read apart from ProjectRow (`Store::worktree_setup`).
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE projects ADD COLUMN setup_copy TEXT;
ALTER TABLE projects ADD COLUMN setup_command TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (221);
