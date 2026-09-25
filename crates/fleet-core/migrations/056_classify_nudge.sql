-- Work graph M4.6: the opt-in classification nudge fires at most once per
-- conversation. When it did is stamped on the conversation it was sent in;
-- NULL means not yet. Guarded in `schema.rs` (ALTER TABLE ADD COLUMN is not
-- idempotent).
ALTER TABLE conversations ADD COLUMN classify_nudged_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (56);
