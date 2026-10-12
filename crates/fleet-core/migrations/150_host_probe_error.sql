-- Review r13: an offline host could not say why, nor when it last answered.
-- The reconcile pass stamps `last_pinged_at` on every probe, failed ones
-- included (down_hosts and the event diff rely on that), so it is not "last
-- answered". These columns are:
--
-- last_reachable_at       unix secs of the last probe the host answered
--                         (moves with last_pinged_at while reachable, stays
--                         put while it is not). NULL: never answered.
-- last_probe_error_code   the failed probe's IpcError code (E_SSH_TIMEOUT …)
-- last_probe_error        its message, cut short. Both cleared by the next
--                         probe the host answers.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs. The migration runs in
-- one transaction, so the three columns land together.
ALTER TABLE hosts ADD COLUMN last_reachable_at INTEGER;
ALTER TABLE hosts ADD COLUMN last_probe_error_code TEXT;
ALTER TABLE hosts ADD COLUMN last_probe_error TEXT;

-- A host reachable now last answered at its last ping.
UPDATE hosts SET last_reachable_at = last_pinged_at WHERE reachable = 1;

INSERT OR IGNORE INTO schema_version (version) VALUES (150);
