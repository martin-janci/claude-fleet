-- PR shepherd, step 3 (docs/superpowers/specs/2026-10-08-pr-shepherd-design.md):
-- the merge queue's record. One row per pushed commit the shepherd tried to
-- merge, so a head is tried at most once and a project's last merge spaces
-- the next one.
--   outcome  merged | skipped:<why> | failed:<error>
CREATE TABLE IF NOT EXISTS pr_shepherd_merges (
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  head_oid    TEXT NOT NULL,
  project_id  INTEGER NOT NULL,
  pr_url      TEXT NOT NULL,
  at          INTEGER NOT NULL,
  outcome     TEXT NOT NULL,
  PRIMARY KEY (session_id, head_oid)
);
CREATE INDEX IF NOT EXISTS pr_shepherd_merges_project
  ON pr_shepherd_merges (project_id, at);

INSERT OR IGNORE INTO schema_version (version) VALUES (142);
