-- Automatic account switching (docs/accounts.md, *Switch automatically*):
-- a person's own answer to "when my session's account runs out, resume it
-- under the login on its host with the most headroom". On by default, so
-- the fleet-wide `accounts.auto_switch` (off by default) is the one switch
-- that turns the feature on; each person, the owner included, can then turn
-- it off for their own sessions. Unowned (unclaimed) sessions follow the
-- personal owner's answer.
ALTER TABLE people ADD COLUMN auto_switch_account INTEGER NOT NULL DEFAULT 1;
INSERT OR IGNORE INTO schema_version (version) VALUES (185);
