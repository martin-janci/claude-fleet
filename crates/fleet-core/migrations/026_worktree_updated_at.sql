-- When a worktree row was last written (unix milliseconds), stamped by every
-- upsert (the project scan and the EnterWorktree hooks). NULL for rows
-- written before this migration.
--
-- The remote worktree prune (`service::worktree_prune`) records when its
-- probe started and, re-checking under the store lock, skips any row written
-- after that: the probe judged the path before a hook re-created it, so the
-- row is live again. Milliseconds, so a re-creation in the same second as the
-- probe start still counts as newer.
ALTER TABLE worktrees ADD COLUMN updated_at_ms INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (26);
