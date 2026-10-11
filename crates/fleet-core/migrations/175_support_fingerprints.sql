-- The support agent's fingerprint records (design
-- docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md,
-- part C, step 7): one row per bug fingerprint a rule's investigations
-- found, so a repeat inside the incident window is linked to the bug the
-- first one filed or matched instead of filing another.
--   scope        the rule's scope (`jira:<tracker id>:<PROJECT>`)
--   fingerprint  sha256(exception class | normalised message | app frames), hex
--   bug_key      the Jira bug it was filed as or linked to, once written
--   sample       the error as the scanner found it (JSON), for a scan job
CREATE TABLE IF NOT EXISTS support_fingerprints (
  scope         TEXT NOT NULL,
  fingerprint   TEXT NOT NULL,
  exception     TEXT NOT NULL,
  bug_key       TEXT,
  sample        TEXT,
  first_job_id  INTEGER,
  last_job_id   INTEGER,
  seen          INTEGER NOT NULL DEFAULT 1,
  first_seen    INTEGER NOT NULL,
  last_seen     INTEGER NOT NULL,
  PRIMARY KEY (scope, fingerprint)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (175);
