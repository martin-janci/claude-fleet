-- A project's GitHub origin (`service::projects` scan, `service::add_project`).
--
-- projects
--   github_origin  the checkout's `origin` URL when it names a github.com
--                  repository (`repo_url::github_origin`); NULL when there
--                  is no origin, it is not on GitHub, or it was not read
--                  yet. A row's owner/repo is NOT that identity: an adopted
--                  checkout without a GitHub origin is `local/<dir>`, and a
--                  scanned one is named after its directories whatever its
--                  remote is. New session on another fleet clones from this
--                  URL, and offers no clone without it.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE projects ADD COLUMN github_origin TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (207);
