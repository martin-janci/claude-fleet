-- Forbid an account for a whole org (docs/accounts.md, *Keep an account off
-- a project*): migration 217's block, for every project of an org at once,
-- today's and those added later. The rules are in
-- `service::project_accounts`.
--
--   account_org_blocks  one row per (account, org) pair a person forbade: a
--                       project whose org (its owner, repo or path rule,
--                       `Store::project_org`) is the org is treated as if
--                       the account were forbidden for it. `account_uuid`
--                       has no foreign key, as in 217. A removed org takes
--                       its rows with it (`Store::remove_org`).
CREATE TABLE IF NOT EXISTS account_org_blocks (
  account_uuid TEXT NOT NULL,
  org_id       INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  PRIMARY KEY (account_uuid, org_id)
);

CREATE INDEX IF NOT EXISTS account_org_blocks_by_org
  ON account_org_blocks (org_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (218);
