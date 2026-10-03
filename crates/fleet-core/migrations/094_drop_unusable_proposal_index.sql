-- Drop an index no query can use.
--
-- 086 added `idx_work_items_proposals ON work_items(parent_id) WHERE
-- proposal_state = 'proposed'`. A partial index is only usable when a query's
-- WHERE clause implies the index's own predicate, and nothing in the tree
-- writes one: the two SQL readers of the column
-- (`store::work::recent_local_work_items` and `store::work_local`) both ask
-- `COALESCE(proposal_state, '') NOT IN ('proposed', 'rejected')`, which is the
-- opposite set and not a literal equality SQLite can match; the open-proposal
-- count in `store::work_tasks` filters in Rust over rows already read; and
-- `decide_proposal` is keyed by the primary key. So it has only ever cost
-- writes to `work_items` and bytes on disk.
--
-- `IF EXISTS`: a database created after this migration never had it.
DROP INDEX IF EXISTS idx_work_items_proposals;

INSERT OR IGNORE INTO schema_version (version) VALUES (94);
