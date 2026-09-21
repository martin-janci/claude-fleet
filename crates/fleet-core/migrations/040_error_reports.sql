-- Error reports collected from every participant (spec
-- 2026-09-21-hub-error-channel-design.md). Bounded by reports.max_rows
-- (pruned on insert) and reports.max_age_secs (swept on the tick).
CREATE TABLE IF NOT EXISTS error_reports (
  id          INTEGER PRIMARY KEY,
  received_at INTEGER NOT NULL,
  at          INTEGER NOT NULL,
  origin      TEXT    NOT NULL,
  level       TEXT    NOT NULL,
  component   TEXT    NOT NULL,
  code        TEXT,
  message     TEXT    NOT NULL,
  context     TEXT,
  truncated   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS error_reports_recent
  ON error_reports(received_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS error_reports_by_origin
  ON error_reports(origin, received_at DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (40);
