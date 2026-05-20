-- Migration 003: normalized accounts + per-host account FK.
-- Step 2 of the multi-host iteration (account model). Auto-populated from
-- each host's ~/.claude.json oauthAccount during probe. Nullable FK on
-- hosts so a host without claude installed or logged in stays usable.

CREATE TABLE IF NOT EXISTS accounts (
  uuid                TEXT PRIMARY KEY,
  email               TEXT,
  display_name        TEXT,
  organization_name   TEXT,
  organization_uuid   TEXT,
  seat_tier           TEXT,
  last_seen_at        INTEGER
);

ALTER TABLE hosts ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
