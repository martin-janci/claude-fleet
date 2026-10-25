-- The evidence retry's per-pass probe (`Store::runs_with_unread_evidence`,
-- review r10).
--
-- `report::retry_unread_evidence` asks every missions pass (20 s), under
-- the store lock, for the finished runs of the last EVIDENCE_RETRY_SECS
-- whose git read failed. No index covered `finished_at`, so each pass
-- walked every done run the store ever recorded (`idx_tasks_state`), and
-- runs are never pruned. Partial on the query's own predicates, so it
-- holds the failed reads only, and keyed on (state, finished_at) so the
-- planner takes it over `idx_tasks_state` and the window bounds the walk.
CREATE INDEX IF NOT EXISTS idx_tasks_unread_evidence
    ON tasks(state, finished_at)
    WHERE work_item_id IS NOT NULL
      AND json_extract(evidence_json, '$.error') IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (208);
