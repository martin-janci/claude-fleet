-- Assets M5 (Rulings R8): when an apply or a person decided a changeset
-- item, Unix MILLISECONDS; NULL while pending and on every item decided
-- before this migration. `rejected_rollouts` orders decisions by it. ADD
-- COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE changeset_items ADD COLUMN decided_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (97);
