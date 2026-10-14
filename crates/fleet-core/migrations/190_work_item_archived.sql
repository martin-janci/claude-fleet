-- 190: `work_items.archived_at`, when a person archived the ticket or task
-- (NULL = not archived). An archived item leaves the ticket views (`mine`,
-- `sprint`, `recent`, a tracker filter) for the `archived` view, and the
-- Work tree counts it archived, so the Archived switch hides it, unless a
-- session is active on it. Unarchive clears it. Stored on the hub, so every
-- device sees it; a tracker sync never touches it.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE work_items ADD COLUMN archived_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (190);
