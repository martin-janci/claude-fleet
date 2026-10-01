-- The warning a DEGRADED provisioning left behind, kept per host.
--
-- `provision_hosts` / `provision_content_only` can deliver the content and
-- still fail part way — the `ag` launcher did not install, say. Until now that
-- warning was returned to the caller and then lost: the unattended path wrote
-- one `tracing::warn!` and the interactive path a transient `detail` string,
-- so an operator who was not watching the console had no way to learn of it.
-- Cleared on the next clean run. NULL = nothing wrong with the last run.
--
-- Pairs with `provision_fingerprint` being cleared on a degraded run: the
-- fingerprint makes the host `provision_stale` so it is retried, and this says
-- WHY, in `fleet_health` and the host's Attention row.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN provision_warning TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (92);
