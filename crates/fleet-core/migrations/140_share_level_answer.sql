-- Share level Answer (Orbit Fleet 11.7): a grant between `watch` and `drive`
-- that may answer the dialog on a session's pane (a numbered option, Enter,
-- Escape, Tab) and type nothing else. `store/session_grants.rs` has the rule;
-- `mcp::tools::messaging::send_prompt` has the gate.
--
-- The level vocabulary is a CHECK on `session_grants.level` (100), and
-- SQLite cannot change a CHECK in place, so the table is rebuilt: copy the
-- rows aside, drop, create it again under its own name with the wider CHECK,
-- copy back (ids preserved, so the audit trail keeps its ids), and recreate
-- the three indexes exactly as 100 wrote them. Not 024's create-new-then-
-- RENAME: `ALTER TABLE … RENAME` re-parses every trigger in the schema, and
-- on a database whose migrations were applied out of order (the branch
-- databases `store::schema`'s tests rebuild) a `sessions` trigger naming a
-- column that database does not have yet fails the rename. `Store::migrate`
-- runs this with foreign keys OFF and requires an empty
-- `PRAGMA foreign_key_check` before the commit; the `already_applied` guard
-- (`session_grants_has_answer`) keeps a re-run from rebuilding again.
CREATE TEMP TABLE session_grants_before_answer AS SELECT * FROM session_grants;

DROP TABLE session_grants;

CREATE TABLE session_grants (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  person_id   INTEGER,
  org_id      INTEGER,
  level       TEXT    NOT NULL CHECK (level IN ('watch', 'answer', 'drive')),
  granted_by  INTEGER NOT NULL,
  granted_at  INTEGER NOT NULL,
  revoked_at  INTEGER,
  CHECK ((person_id IS NULL) <> (org_id IS NULL))
);

INSERT INTO session_grants
  (id, session_id, person_id, org_id, level, granted_by, granted_at, revoked_at)
  SELECT id, session_id, person_id, org_id, level, granted_by, granted_at, revoked_at
    FROM session_grants_before_answer;

DROP TABLE session_grants_before_answer;

CREATE UNIQUE INDEX IF NOT EXISTS idx_session_grants_live
  ON session_grants(session_id, COALESCE(person_id, 0), COALESCE(org_id, 0))
  WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_session_grants_person
  ON session_grants(person_id)
  WHERE revoked_at IS NULL AND person_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_session_grants_session
  ON session_grants(session_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (140);
