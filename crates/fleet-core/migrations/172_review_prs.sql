-- The PR reviewer (docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md,
-- step 3): what the reviewer knows of each pull request it watches, and the
-- reviews it wrote. The jobs themselves are agent_jobs (171).
--
-- review_prs: one row per pull request of a watched repo, keyed by repo
-- and number, so a rule granted again finds its history.
--   status       pending_review | reviewed | changes_pushed | skipped |
--                closed | merged
--   skip_reason  draft | wip_title | diff_too_large | empty_diff
--   last_push_at when the reviewer first saw the current head (the
--                debounce counts from it)
--   skipped_at_sha / skip_diff_lines  a size skip's head and size, so a
--                new push is looked at again
--   review_id / review_verified_at  the posted review GitHub holds, and
--                when the reviewer last saw it there (a deleted or
--                dismissed one is reviewed again)
CREATE TABLE IF NOT EXISTS review_prs (
  repo               TEXT NOT NULL,
  number             INTEGER NOT NULL CHECK (number > 0),
  status             TEXT NOT NULL CHECK (status IN
                       ('pending_review', 'reviewed', 'changes_pushed', 'skipped',
                        'closed', 'merged')),
  skip_reason        TEXT CHECK (skip_reason IS NULL OR skip_reason IN
                       ('draft', 'wip_title', 'diff_too_large', 'empty_diff')),
  title              TEXT NOT NULL DEFAULT '',
  author             TEXT,
  is_draft           INTEGER NOT NULL DEFAULT 0,
  head_sha           TEXT NOT NULL,
  base_ref           TEXT NOT NULL DEFAULT '',
  head_ref           TEXT NOT NULL DEFAULT '',
  last_push_at       INTEGER NOT NULL,
  last_reviewed_sha  TEXT,
  last_reviewed_at   INTEGER,
  skipped_at_sha     TEXT,
  skip_diff_lines    INTEGER,
  first_seen_at      INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL,
  closed_at          INTEGER,
  review_id          INTEGER,
  review_verified_at INTEGER,
  PRIMARY KEY (repo, number)
);
CREATE INDEX IF NOT EXISTS review_prs_status ON review_prs (repo, status);

-- review_records: one review the reviewer wrote, posted or not.
--   review     the placed review as JSON (verdict, summary, inline
--              comments, the findings that went to the body)
--   posted     1 once GitHub has it; review_id is GitHub's id then
CREATE TABLE IF NOT EXISTS review_records (
  id          INTEGER PRIMARY KEY,
  repo        TEXT NOT NULL,
  number      INTEGER NOT NULL,
  sha         TEXT NOT NULL,
  job_id      INTEGER REFERENCES agent_jobs(id) ON DELETE SET NULL,
  verdict     TEXT NOT NULL CHECK (verdict IN ('APPROVE', 'REQUEST_CHANGES', 'COMMENT')),
  review      TEXT NOT NULL CHECK (json_valid(review)),
  posted      INTEGER NOT NULL DEFAULT 0,
  review_id   INTEGER,
  created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS review_records_pr ON review_records (repo, number, created_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (172);
