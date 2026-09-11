-- Parent-directory fingerprints for automatic workspace repair
-- (service::repair, VanishedGuard::fingerprint_matches).
--
-- Whenever a probe finds a registered worktree HEALTHY, the `dev:inode` of
-- its parent directory (`stat -L`, the canonical parent) is recorded here,
-- keyed by host and canonical worktree path. An automatic removal of that
-- worktree's stale registration later requires the current parent `dev:inode`
-- to equal the recorded one: an unmounted mountpoint, a remount or a
-- replaced parent directory shows a different inode (or none), so it stays
-- explicit-only.
CREATE TABLE IF NOT EXISTS worktree_parent_fingerprints (
  host_alias  TEXT NOT NULL,
  wt_path     TEXT NOT NULL,
  parent_fp   TEXT NOT NULL,
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, wt_path)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (23);
