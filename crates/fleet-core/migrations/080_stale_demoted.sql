-- Stale-working acknowledgement, final review: `stale_working_at` did two
-- jobs — the attention reason `stale_working`, and the only input to the
-- reconcile's `stale_working_veto`, which keeps the cached `claude agents`
-- status (`working`, e.g. live subagents) from promoting a demoted row back
-- to `working`. An attach or `reconcile.stale_working_ttl_secs` ends the
-- reason; neither may lift the demotion. `stale_demoted_at` is the veto's
-- own memory: set with the stamp by the tick's stale-working rule, cleared
-- by every hook, by a pane that shows a live turn and by a row that is
-- `working` / `blocked` again — never by an attach or the TTL.
--
-- Not a `SessionRow` field and not on the wire, so migration 065's
-- `sessions_row_version_bump` does not watch it: a change to it alone is
-- not a client-visible change.
ALTER TABLE sessions ADD COLUMN stale_demoted_at INTEGER;

-- A row already demoted keeps its veto: the backfill
-- (`stale_demoted_at = stale_working_at` where only the stamp is set) is
-- `Store::backfill_stale_demoted`, run by `migrate()` after the migrations
-- and the collision repair. Not here: any UPDATE of `sessions` compiles
-- 065's row_version trigger, which names `lost_reason` — and on a
-- conversation-branch database that column only exists once
-- `repair_skipped_main_migrations` has run.

INSERT OR IGNORE INTO schema_version (version) VALUES (80);
