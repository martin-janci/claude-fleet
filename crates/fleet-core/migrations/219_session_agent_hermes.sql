-- 219: `sessions.agent` admits `hermes`, Nous Research's Hermes Agent CLI
-- (`agent_adapter::Hermes`).
--
-- Migration 121 wrote the allowed agents as a column CHECK, and SQLite has
-- no ALTER for a CHECK. Rebuilding `sessions` would mean restating its ~90
-- columns, 4 indexes and 14 triggers, so this takes the route SQLite's own
-- ALTER TABLE documentation gives for loosening a CHECK: rewrite the
-- table's stored CREATE text with `writable_schema`, then `RESET` so this
-- connection re-reads its schema before the next statement. Adding a value
-- to the IN list only admits more rows, so every row already stored still
-- satisfies it and nothing is copied.
--
-- `replace` makes the edit idempotent: a second run (a test rolling the
-- recorded version back) finds the old list gone and changes nothing.
PRAGMA writable_schema = ON;

UPDATE sqlite_master
   SET sql = replace(
         sql,
         'CHECK (agent IN (''claude'', ''codex'', ''agy'', ''shell''))',
         'CHECK (agent IN (''claude'', ''codex'', ''agy'', ''hermes'', ''shell''))'
       )
 WHERE type = 'table' AND name = 'sessions';

PRAGMA writable_schema = RESET;

-- Fail the migration (and roll it back) when the edit above matched
-- nothing, rather than leave a schema that still refuses `hermes`.
CREATE TEMP TABLE migration_219_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO migration_219_check (ok)
  SELECT instr(sql, '''hermes''') > 0 FROM sqlite_master
   WHERE type = 'table' AND name = 'sessions';
DROP TABLE migration_219_check;

INSERT OR IGNORE INTO schema_version (version) VALUES (219);
