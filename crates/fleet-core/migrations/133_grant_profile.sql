-- Redesign 8.7, account-aware automation: a mission grant names the login
-- its runs bill (a credential profile on the run's host; NULL = the host's
-- own), so the loop can check that account's usage before a run.
ALTER TABLE orchestration_grants ADD COLUMN profile TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (133);
