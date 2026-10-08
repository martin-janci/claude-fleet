-- Orbit Fleet redesign step 12.4: which agent CLIs a host has on its PATH,
-- so the New session picker enables an agent only where it can run. The
-- health probe asks `command -v` for each of claude, codex, agy and gemini
-- every pass and stores the ones found, as a JSON string array.
-- NULL = never sampled, or the host could not tell; '[]' = none found.
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN agents_on_path TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (134);
