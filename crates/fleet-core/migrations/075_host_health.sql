-- Host identity & health, task 2: what the reconcile probe now reads from
-- every reachable host each pass (hosts F4, ux F-14 — two hosts sat at 98 %
-- disk with no signal anywhere), plus two liveness stamps nothing else
-- carried: the last accepted hook from the host's token (hosts F9) and the
-- fleet-agent version its hello reported (hosts F5). All nullable: an SSH
-- host has no agent, an older store has no samples.
ALTER TABLE hosts ADD COLUMN disk_home_free_kb INTEGER;
ALTER TABLE hosts ADD COLUMN disk_home_total_kb INTEGER;
ALTER TABLE hosts ADD COLUMN disk_tmp_free_kb INTEGER;
ALTER TABLE hosts ADD COLUMN load_1m REAL;
ALTER TABLE hosts ADD COLUMN mem_avail_kb INTEGER;
ALTER TABLE hosts ADD COLUMN uptime_secs INTEGER;
ALTER TABLE hosts ADD COLUMN health_at INTEGER;
ALTER TABLE hosts ADD COLUMN last_hook_at INTEGER;
ALTER TABLE hosts ADD COLUMN agent_version TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (75);
