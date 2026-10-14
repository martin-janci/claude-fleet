-- Automatic account switching, part two (docs/accounts.md, *Switch
-- automatically*): a person's answer to "after my session is switched, tell
-- it to carry on". Off by default: typing a prompt into someone's session is
-- a step past resuming it. When on, a switch whose session's last turn was
-- cut off by the limit queues "continue" as a deferred prompt.
ALTER TABLE people ADD COLUMN auto_switch_continue INTEGER NOT NULL DEFAULT 0;
INSERT OR IGNORE INTO schema_version (version) VALUES (187);
