-- Assets M6 (Rulings R1): what a host-writing card left undone on one
-- item's host, as JSON {held: [{kind, name, why}], note?}: the copies it
-- held back ("sync it yourself") and a skipped or failed host's line. NULL
-- on items applied cleanly and on every item recorded before this
-- migration. ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE changeset_items ADD COLUMN outcome TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (102);
