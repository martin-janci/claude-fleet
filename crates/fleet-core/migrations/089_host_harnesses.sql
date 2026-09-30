-- Multi-harness F3a: which harnesses the asset catalog syncs on a host.
-- NULL = auto: Claude always, Codex where the scan finds it (the codex CLI on
-- PATH or ~/.codex) or fleet already manages Codex assets there. A JSON array
-- (e.g. ["claude","codex"]) is an explicit choice made with
-- set_host_harnesses; it always contains "claude". ADD COLUMN is not
-- idempotent: guarded in schema.rs.
ALTER TABLE hosts ADD COLUMN harnesses TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (89);
