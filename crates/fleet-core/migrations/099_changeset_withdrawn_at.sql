-- Assets M6 (Rulings R3): when the system withdrew a card (Unix SECONDS),
-- so a withdrawn card is pruned a week after its withdrawal, not after its
-- creation. NULL on every other card and on cards withdrawn before this
-- migration (pruning falls back to created_at for those). ADD COLUMN is
-- not idempotent: guarded in schema.rs.
ALTER TABLE changesets ADD COLUMN withdrawn_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (99);
