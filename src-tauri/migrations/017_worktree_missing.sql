-- A worktree whose working directory has gone missing on disk is marked here
-- with the unix time it was first observed missing (NULL = present). Drives the
-- two-phase mark→auto-prune lifecycle, mirroring sessions.lost_at.
ALTER TABLE worktrees ADD COLUMN missing_since INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (17);
