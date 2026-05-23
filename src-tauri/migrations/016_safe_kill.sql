-- Safe-kill flow: user asks a session to persist its work safely (commit +
-- push to main, or open a PR) before fleet deletes the worktree and kills the
-- tmux session. State transitions, driven by safe_kill_session() and the
-- Stop-hook marker check:
--
--   NULL --safe_kill_session()--> 'requested'
--   'requested' --Stop hook + READY marker--> 'ready'  (then auto-deleted)
--   'requested' --Stop hook + FAILED marker--> 'failed' (session stays alive)
--   'failed' --safe_kill_session() retry--> 'requested'
--
-- `safe_kill_nonce` is a per-request random tag embedded in the prompt so the
-- marker scan can distinguish the assistant's emission from the prompt echo
-- still visible in the pane scrollback. `safe_kill_detail` carries the
-- one-line failure reason Claude reports.
ALTER TABLE sessions ADD COLUMN safe_kill_state TEXT;
ALTER TABLE sessions ADD COLUMN safe_kill_nonce TEXT;
ALTER TABLE sessions ADD COLUMN safe_kill_detail TEXT;
ALTER TABLE sessions ADD COLUMN safe_kill_requested_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (16);
