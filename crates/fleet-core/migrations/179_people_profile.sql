-- Identity sub-project 1 (spec docs/superpowers/specs/2026-10-11-identity-realm-model-design.md §1):
-- a person is a Keycloak user. `uuid` is the user's `id` in a realm export
-- and in tokens the hub will issue; the integer id stays internal. It is
-- filled by `Store::backfill_person_uuids` (a v4 uuid needs the uuid crate),
-- which runs on every open, so a row inserted by an older binary gets one too.
-- attributes  JSON object of string arrays, Keycloak's user attributes.
ALTER TABLE people ADD COLUMN uuid TEXT;
ALTER TABLE people ADD COLUMN email TEXT;
ALTER TABLE people ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
ALTER TABLE people ADD COLUMN first_name TEXT;
ALTER TABLE people ADD COLUMN last_name TEXT;
ALTER TABLE people ADD COLUMN attributes TEXT NOT NULL DEFAULT '{}';
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_uuid ON people(uuid) WHERE uuid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_live_email
  ON people(lower(email)) WHERE email IS NOT NULL AND disabled_at IS NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (179);
