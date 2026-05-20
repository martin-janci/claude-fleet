-- Migration 002: per-host SSH alias.
-- Step 1 of the multi-host iteration. The `local` host stays special
-- (ssh_alias=NULL); registered remote hosts store the name they have
-- in the user's ~/.ssh/config (e.g. "mefistos") so SshClient knows
-- what to pass to `ssh`.

ALTER TABLE hosts ADD COLUMN ssh_alias TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (2);
