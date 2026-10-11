-- Single sign-on (Keycloak / OIDC, `mcp::oidc`): which person an identity
-- provider's account is. One row per (issuer, subject) — the pair OIDC says
-- identifies an account; a username or e-mail address is a claim the
-- provider may let its user change, so nothing keys on it.
--
-- A person may hold several (two realms, a re-created account); an account
-- is exactly one person. `person_id` has no foreign key, the migration-066
-- rationale: a removed person must leave a row pointing at an id nothing
-- has (fail closed), never cascade into a widening. A link to a DISABLED
-- person refuses the sign-in rather than minting someone new.
CREATE TABLE IF NOT EXISTS person_identities (
  issuer        TEXT    NOT NULL,
  subject       TEXT    NOT NULL,
  person_id     INTEGER NOT NULL,
  created_at    INTEGER NOT NULL,
  last_login_at INTEGER,
  PRIMARY KEY (issuer, subject)
);
CREATE INDEX IF NOT EXISTS idx_person_identities_person ON person_identities(person_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (164);
