-- 076: `sessions.pane_working_at`, the last reconcile pass whose pane
-- capture showed a live turn (the spinner's "esc to interrupt"). The
-- stale-working sweep (065, lifecycle F2) demotes a `working` row nothing
-- has moved for `reconcile.stale_working_secs`; one tool call running longer
-- than that fires no hook, grows no transcript and moves no tmux
-- `session_activity` (that tracks client input, not pane output). Without
-- this stamp the sweep demoted such a row every tick and every agents pass
-- lifted it again. Per-pass bookkeeping, not a `SessionRow` field: the
-- row-version trigger does not watch it. One ADD COLUMN, guarded in
-- schema.rs.
ALTER TABLE sessions ADD COLUMN pane_working_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (76);
