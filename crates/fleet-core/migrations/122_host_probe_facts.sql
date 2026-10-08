-- Orbit Fleet redesign step 4.6, the Hosts page: what the reconcile probe
-- now also reads from every reachable host. All nullable: an older store
-- has no samples, and `local` has no network round trip.
--   cpu_count     online CPUs
--   mem_total_kb  physical memory
--   boot_at       boot time as the host states it (unix seconds)
--   latency_ms    round trip of an empty command over the host's SSH
--   worktree_kb   disk held by fleet's worktrees on the host (`du -sk`)
--   worktree_at   when that size was last asked for; a slow `du` is not
--                 retried before the next interval either way
ALTER TABLE hosts ADD COLUMN cpu_count INTEGER;
ALTER TABLE hosts ADD COLUMN mem_total_kb INTEGER;
ALTER TABLE hosts ADD COLUMN boot_at INTEGER;
ALTER TABLE hosts ADD COLUMN latency_ms INTEGER;
ALTER TABLE hosts ADD COLUMN worktree_kb INTEGER;
ALTER TABLE hosts ADD COLUMN worktree_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (122);
