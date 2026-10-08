-- Pull requests (redesign step 6.4): every PR a session's branch has had,
-- upserted by reconcile from the same `gh pr view` probe that fills
-- `sessions.pr_url` / `ci_status` / `pr_evidence`, and kept after the
-- session is gone, so Work › Pull requests can list merged and closed ones.
-- url             the PR's https URL; the identity
-- repo, number    parsed from the URL (`owner/name`, the PR number)
-- state           OPEN | CLOSED | MERGED, as gh spells it
-- ci_status       passing | failing | pending, or NULL without checks
-- review_decision APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED, or NULL
-- merged_at       unix secs: gh's `mergedAt`, else the pass that first saw
--                 it merged
-- session_id      the session that opened it: the first one seen on it,
--                 never moved by a later session on the same branch. Not a
--                 foreign key: the PR outlives the session row.
-- session_name, host_alias  that session's name and host when first seen
CREATE TABLE IF NOT EXISTS pull_requests (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  url             TEXT    NOT NULL UNIQUE,
  repo            TEXT,
  number          INTEGER,
  title           TEXT,
  head_ref        TEXT,
  state           TEXT    NOT NULL DEFAULT 'OPEN',
  draft           INTEGER NOT NULL DEFAULT 0,
  ci_status       TEXT,
  review_decision TEXT,
  merge_state     TEXT,
  merged_at       INTEGER,
  session_id      INTEGER,
  session_name    TEXT,
  host_alias      TEXT,
  project_id      INTEGER,
  first_seen_at   INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pull_requests_state_updated
  ON pull_requests (state, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_pull_requests_session
  ON pull_requests (session_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (128);
