-- Person-scoped session environment variables: `env_vars.scope` gains
-- 'person', whose `scope_key` is the `people.id` (as text) of the person who
-- OWNS the session at launch (`sessions.owner_person_id`). Precedence is
-- global < host < project < person < session (`store/env_vars.rs`).
--
-- SQLite cannot change a CHECK constraint in place, so the table is rebuilt:
-- copy the rows aside, drop, re-create under the new CHECK, copy back (ids
-- preserved). Not create-new-then-RENAME as 024 does: a RENAME re-parses
-- every trigger in the schema and fails on any that names a column a branch
-- database lacks, which this rebuild has no reason to depend on. The
-- session-delete trigger lives on `sessions` and names `env_vars` only in its
-- body, so it survives the drop untouched. Guarded by
-- `env_vars_has_person_scope` (`store/schema.rs`), so a re-run does not
-- rebuild again.
DROP TABLE IF EXISTS env_vars_v192_copy;

CREATE TABLE env_vars_v192_copy AS SELECT * FROM env_vars;

DROP TABLE env_vars;

CREATE TABLE env_vars (
  id             INTEGER PRIMARY KEY,
  scope          TEXT NOT NULL CHECK (scope IN ('global','host','project','person','session')),
  scope_key      TEXT NOT NULL,
  name           TEXT NOT NULL,
  value          TEXT,
  credential_ref TEXT,
  secret         INTEGER NOT NULL DEFAULT 0,
  updated_at     INTEGER NOT NULL,
  UNIQUE (scope, scope_key, name),
  CHECK ((value IS NULL) <> (credential_ref IS NULL)),
  CHECK (scope <> 'global' OR scope_key = '')
);

INSERT INTO env_vars (id, scope, scope_key, name, value, credential_ref, secret, updated_at)
  SELECT id, scope, scope_key, name, value, credential_ref, secret, updated_at
  FROM env_vars_v192_copy;

DROP TABLE env_vars_v192_copy;

-- A person's rows go with the person row. People are normally disabled, not
-- deleted (their sessions keep pointing at them), so this is hygiene for the
-- paths that do delete one.
CREATE TRIGGER IF NOT EXISTS trg_env_vars_on_person_delete
AFTER DELETE ON people
BEGIN
  DELETE FROM env_vars WHERE scope = 'person' AND scope_key = CAST(old.id AS TEXT);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (192);
