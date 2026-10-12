-- Routine event triggers on pull requests (Orbit Fleet M15 step G2.4): an
-- event routine may fire on a pull request's review, CI or merge
-- (`service::routines::PR_EVENTS`, written to the timeline of the session
-- that opened the PR when its `pull_requests` row changes), narrowed by
-- repo and author, and held to a rate.
--   event_repo       only PRs of this repo: `owner/name`, or `name` of any
--                    owner (compared without case); NULL = any repo. PR
--                    events only.
--   event_author     me (or NULL) | anyone: a PR opened by a session of the
--                    routine's owner, or by any session of its org. A
--                    session event is always its owner's.
--   event_rate_secs  at most one run per subject (a PR, or a session for a
--                    session event) in this many seconds; NULL = no limit.
--                    A fire inside it is dropped, not recorded.
ALTER TABLE routines ADD COLUMN event_repo TEXT;
ALTER TABLE routines ADD COLUMN event_author TEXT CHECK (event_author IN ('me', 'anyone'));
ALTER TABLE routines ADD COLUMN event_rate_secs INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (156);
