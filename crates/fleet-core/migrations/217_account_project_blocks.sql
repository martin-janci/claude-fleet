-- Forbid an account for a project (docs/accounts.md, *Keep an account off a
-- project*): the Claude accounts a project's sessions must never bill. The
-- rules are in `service::project_accounts`.
--
--   account_project_blocks  one row per (account, project) pair a person
--                           forbade: no start, resume, switch, routine run or
--                           mission run of the project picks a login on the
--                           account, and a start that names one is refused
--                           with E_ACCOUNT_FORBIDDEN. `account_uuid` has no
--                           foreign key: accounts are never deleted, and a
--                           block may name an account before any host has
--                           reported it again. A removed project takes its
--                           rows with it.
CREATE TABLE IF NOT EXISTS account_project_blocks (
  account_uuid TEXT NOT NULL,
  project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  PRIMARY KEY (account_uuid, project_id)
);

CREATE INDEX IF NOT EXISTS account_project_blocks_by_project
  ON account_project_blocks (project_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (217);
