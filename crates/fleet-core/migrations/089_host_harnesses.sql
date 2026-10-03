-- Multi-harness F3a: which harnesses the asset catalog syncs on a host.
-- NULL = auto: Claude always, Codex where the scan finds it or fleet already
-- manages Codex assets there. "Finds it" is the probe in
-- service/catalog/harness/codex.rs: the codex CLI on PATH, ~/.codex/auth.json,
-- or a ~/.codex/sessions directory — NOT a bare ~/.codex, which a stray config
-- file alone would create. A JSON array
-- (e.g. ["claude","codex"]) is an explicit choice made with
-- set_host_harnesses; it always contains "claude". ADD COLUMN is not
-- idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN harnesses TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (89);
