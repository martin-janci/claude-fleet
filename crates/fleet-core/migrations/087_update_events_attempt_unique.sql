-- 079's `UNIQUE (target, attempt, phase)` also covered attempt-less reports
-- (`attempt = ''`: checking / available / idle), so every later `checking`
-- of a target was ignored as a "replay" of the first until retention dropped
-- it. Only a real attempt's transitions are replays; the key becomes a
-- partial unique index, and the inline constraint (which SQLite cannot drop)
-- goes with a table rebuild. Safe to re-run: the rebuild starts from a clean
-- `update_events_rebuild` and the indexes are `IF NOT EXISTS`.
-- Rebuilt WITHOUT `ALTER TABLE … RENAME` (as 071): a rename re-parses every
-- trigger in the schema, and on a database whose `main` 034-036 / 065
-- columns are still missing (fixed by `repair_skipped_main_migrations` AFTER
-- the pending migrations) `sessions_row_version_bump` names columns that do
-- not exist yet and the rename fails. Copy out, drop, recreate, copy back.
DROP TABLE IF EXISTS update_events_rebuild;
CREATE TABLE update_events_rebuild (
  id           INTEGER PRIMARY KEY,
  target       TEXT    NOT NULL,
  attempt      TEXT    NOT NULL DEFAULT '',
  phase        TEXT    NOT NULL,
  from_version TEXT,
  to_version   TEXT,
  detail       TEXT,
  error        TEXT,
  at           INTEGER NOT NULL
);
INSERT INTO update_events_rebuild
  (id, target, attempt, phase, from_version, to_version, detail, error, at)
  SELECT id, target, attempt, phase, from_version, to_version, detail, error, at
  FROM update_events;
DROP TABLE update_events;
CREATE TABLE update_events (
  id           INTEGER PRIMARY KEY,
  target       TEXT    NOT NULL,
  attempt      TEXT    NOT NULL DEFAULT '',
  phase        TEXT    NOT NULL,
  from_version TEXT,
  to_version   TEXT,
  detail       TEXT,
  error        TEXT,
  at           INTEGER NOT NULL
);
INSERT INTO update_events
  (id, target, attempt, phase, from_version, to_version, detail, error, at)
  SELECT id, target, attempt, phase, from_version, to_version, detail, error, at
  FROM update_events_rebuild;
DROP TABLE update_events_rebuild;
CREATE INDEX IF NOT EXISTS idx_update_events_at ON update_events(at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_update_events_attempt_phase
  ON update_events(target, attempt, phase) WHERE attempt <> '';

INSERT OR IGNORE INTO schema_version (version) VALUES (87);
