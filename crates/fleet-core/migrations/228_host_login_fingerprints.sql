-- Fingerprints of each Claude login's refresh token on a host, to spot one
-- login copied onto two hosts (or into two config dirs): every OAuth
-- refresh revokes the old refresh token, so two copies log each other out.
-- The health probe hashes `claudeAiOauth.refreshToken` of each
-- `.credentials.json` on the host and prints the first 12 hex digits of its
-- SHA-256; the token never leaves the host. JSON array of
-- {"profile": null | "<name>", "seen": ["<newest>", …]}: the last few
-- fingerprints of each login, newest first, so a copy still matches the
-- original after the original refreshes once (`service::login_conflicts`).
-- NULL = never sampled, or the host could not tell.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN login_fingerprints TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (228);
