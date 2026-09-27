-- Work graph: the webhook nudges of M13.4f (migration 062) were removed
-- again, because decision D13 stays "no". 062 stays in the chain, since a
-- database that ran it must not be refused as newer, and this drops its
-- table: the webhook secrets in it have no reader, and no command can
-- rotate or remove them any more.
DROP TABLE IF EXISTS tracker_webhooks;
INSERT OR IGNORE INTO schema_version (version) VALUES (63);
