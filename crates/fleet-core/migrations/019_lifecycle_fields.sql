-- Wave 2 Track D: session lifecycle + outcome fields.
--
-- idle_since        unix secs when claude_status last entered idle/completed/
--                   stopped; NULL while working/blocked/unknown. Drives the
--                   session GC sweeper (service::gc).
-- stuck_since       unix secs when the current stuck_kind episode began; NULL
--                   when not stuck. A playbook runs at most once per episode.
-- last_playbook_at  unix secs of the last playbook applied to this row.
-- last_prompt       first 200 chars of the most recent prompt sent via
--                   send_prompt / new_bg_session.
-- started_at        unix secs when the fleet created the session (NULL for
--                   sessions discovered from tmux rather than created here).
-- last_turn_at      unix secs of the last Stop hook (turn completed).
-- ci_status         'passing' | 'failing' | 'pending' derived from the PR's
--                   statusCheckRollup, populated with pr_url by reconcile.
ALTER TABLE sessions ADD COLUMN idle_since INTEGER;
ALTER TABLE sessions ADD COLUMN stuck_since INTEGER;
ALTER TABLE sessions ADD COLUMN last_playbook_at INTEGER;
ALTER TABLE sessions ADD COLUMN last_prompt TEXT;
ALTER TABLE sessions ADD COLUMN started_at INTEGER;
ALTER TABLE sessions ADD COLUMN last_turn_at INTEGER;
ALTER TABLE sessions ADD COLUMN ci_status TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (19);
