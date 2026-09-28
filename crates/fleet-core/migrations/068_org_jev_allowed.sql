-- Jev evaluation (decisions D31 / D36): an org's consent to decision-model
-- calls. 1 = the org's redacted texts may be sent to TypeSafe (Jev) when
-- `decide.jev.enabled` and a feature's `decide.jev.<feature>` mode allow
-- it; 0 (the default) = never. Rows with no org follow the global
-- `decide.jev.unassigned` instead. Its own migration (and guard): an
-- ADD COLUMN cannot be re-run.
ALTER TABLE orgs ADD COLUMN jev_allowed INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (68);
