-- Migration 004: cached account binding per session.
-- Step 3 of the multi-host iteration (cross-host session memory). When a
-- new tmux session is discovered, we capture the host's current account_uuid
-- on the session row. Existing rows are NOT rewritten on re-probe — the
-- preservation invariant lets the UI show the account a session was
-- originally created under even if the host later re-auths.

ALTER TABLE sessions ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
