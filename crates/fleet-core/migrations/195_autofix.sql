-- Autofix (docs/superpowers/specs/2026-10-17-autofix-design.md): one row per
-- EPISODE of a detected broken state. `(kind, subject)` is unique while the
-- row is live; a cleared row stays as history and a recurrence opens a new one.
CREATE TABLE IF NOT EXISTS autofix_findings (
  id              INTEGER PRIMARY KEY,
  kind            TEXT NOT NULL,
  subject         TEXT NOT NULL,
  host_alias      TEXT,
  severity        TEXT NOT NULL,
  body            TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'open',
  first_seen_at   INTEGER NOT NULL,
  last_seen_at    INTEGER NOT NULL,
  dismissed_until INTEGER,
  attempts        INTEGER NOT NULL DEFAULT 0,
  last_attempt_at INTEGER,
  last_error      TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS autofix_findings_live
  ON autofix_findings(kind, subject)
  WHERE state IN ('open', 'verifying', 'apply_failed', 'dismissed');
INSERT OR IGNORE INTO schema_version (version) VALUES (195);
