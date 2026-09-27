-- Host identity & health, task 1: when `hosts.claude_version` /
-- `tmux_version` were last read from the host itself. Until now only
-- `probe_host` / `add_host` wrote a fresh version and every reconcile pass
-- wrote the STORED value back, so a row's `last_pinged_at` said "minutes
-- ago" about a version from provisioning day (data-sync F1, hosts F3).
-- NULL = never probed for versions (a row copied from an older store).
ALTER TABLE hosts ADD COLUMN claude_version_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (66);
