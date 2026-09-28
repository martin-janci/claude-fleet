-- Host identity & health, task 6: which content `provision_hosts` last put
-- on the host (`service::provision::fingerprint()`), and when. NULL = an
-- older provisioning whose content is unknown — reported stale.
ALTER TABLE hosts ADD COLUMN provision_fingerprint TEXT;
ALTER TABLE hosts ADD COLUMN provisioned_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (73);
