-- Review r16 (performance, H8): the org half of `Store::grants_for_person`
-- joins a person's org memberships to `session_grants` on `org_id`, and no
-- index covered that column, so every access check for a person in an org
-- scanned the whole grant history, revoked rows included. A partial index
-- holds only the live org grants, the rows that query can return.
CREATE INDEX IF NOT EXISTS idx_session_grants_org
  ON session_grants(org_id)
  WHERE revoked_at IS NULL AND org_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (147);
