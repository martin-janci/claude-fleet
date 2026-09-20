-- Marks the project row `service::operator` creates for the UX agent's own
-- working directory (`~/.claude-fleet/operator`). It is not one of the
-- user's repositories: the project picker hides it, and `refresh_projects`'s
-- stale-rows sweep must never delete it for living outside the projects root
-- and not being rediscovered by a scan — that is its normal shape, not
-- evidence of staleness, exactly as with `adopted` (027). 0/1 as INTEGER
-- (SQLite has no native boolean); every other row defaults to 0.
ALTER TABLE projects ADD COLUMN system INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (38);
