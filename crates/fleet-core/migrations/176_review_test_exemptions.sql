-- The PR reviewer's test exemptions (design
-- docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md,
-- step 8, test debate): a missing-test finding the reviewer conceded in a
-- debate with the pull request's author. Later reviews of the pull request
-- are told not to raise it again.
CREATE TABLE IF NOT EXISTS review_test_exemptions (
  repo         TEXT NOT NULL,
  number       INTEGER NOT NULL,
  path         TEXT NOT NULL,
  line         INTEGER,
  reason       TEXT NOT NULL,
  root_id      INTEGER NOT NULL,
  conceded_at  INTEGER NOT NULL,
  PRIMARY KEY (repo, number, root_id)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (176);
