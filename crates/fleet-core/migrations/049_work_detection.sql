-- Work graph M4 (docs/superpowers/plans/2026-09-24-work-graph-m4-detection.md):
-- detection signals and link explanations.
--
-- sessions.current_branch / current_branch_at
--                the live branch, read from the transcript's `gitBranch` after
--                every Stop (`context::refresh`). A STATE signal (review C10):
--                only its present value is a candidate. Deliberately NOT in
--                reconcile's `ON CONFLICT` list — reconcile never writes it.
-- sessions.pr_signals / pr_signals_at
--                what the PR probe read about the branch's PR, parsed and
--                reduced (head branch, closing refs, keys in the title / body /
--                commit trailers), JSON. The body itself is never stored.
--                Written only by the probe, again outside `ON CONFLICT`.
-- work_links.state  now also `suggested` (a guess a person has not decided).
-- work_links.claude_session_id
--                the conversation window the link was decided (or, for a
--                suggestion, last seen) in; NULL for links older than M4.
-- work_links.strength   explicit | strong | weak (design §0.3 tiers).
-- work_links.rule       the resolver rule that made it (R2 … R8), or NULL.
-- work_links.evidence   JSON array of what was seen, newest last, capped
--                       (denormalised, review C13: never ids of prunable rows).
-- work_links.preselected  1 for a suggestion shown ticked (R3b, a sole key in
--                       a conversation's first prompt).
-- work_links.end_reason   why a live session's link ended (branch_changed …);
--                       NULL for a retirement.
ALTER TABLE sessions ADD COLUMN current_branch TEXT;
ALTER TABLE sessions ADD COLUMN current_branch_at INTEGER;
ALTER TABLE sessions ADD COLUMN pr_signals TEXT;
ALTER TABLE sessions ADD COLUMN pr_signals_at INTEGER;

ALTER TABLE work_links ADD COLUMN claude_session_id TEXT;
ALTER TABLE work_links ADD COLUMN strength TEXT;
ALTER TABLE work_links ADD COLUMN rule TEXT;
ALTER TABLE work_links ADD COLUMN evidence TEXT;
ALTER TABLE work_links ADD COLUMN preselected INTEGER NOT NULL DEFAULT 0;
ALTER TABLE work_links ADD COLUMN end_reason TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (49);
