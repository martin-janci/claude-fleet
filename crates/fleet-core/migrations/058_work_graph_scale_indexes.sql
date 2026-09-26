-- Work graph M12.2 (scale): two indexes the seeded scale tests
-- (`service/work/scale_tests.rs`) showed missing. Index-only; no row changes.
--
-- `recent_ended_work_links` (the sidebar's past work and the Today view's
-- shipped PRs) filters and orders by `ended_at`, and no index had it: every
-- call read all of `work_links` and sorted it. Partial: a live link's
-- `ended_at` is NULL and the query never wants one.
CREATE INDEX IF NOT EXISTS idx_work_links_ended
  ON work_links(ended_at) WHERE ended_at IS NOT NULL;

-- The prompt trigger's loop guard (`recent_handover_bodies`) reads a
-- participant's handovers, delivered or not; 047's partial index covers only
-- the undelivered ones, so every UserPromptSubmit read all of `work_journal`.
CREATE INDEX IF NOT EXISTS idx_work_journal_participant_handover
  ON work_journal(participant_id) WHERE kind = 'handover';

INSERT OR IGNORE INTO schema_version (version) VALUES (58);
