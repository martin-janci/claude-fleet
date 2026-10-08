-- Orbit Fleet redesign step 8.10 (N6): what a finished routine run came to,
-- beside `state` (which says only whether it ran). The rules are in
-- `service::routines::outcome`.
--   outcome         did_work | nothing | failed | needs_person; NULL until
--                   something answers (a run still open, or a done run no
--                   rule could read, which Jev answers once 8.10's use case
--                   is on)
--   outcome_source  exit (the run failed: that always wins) | rule (a fact
--                   on the session: an open question, a PR) | jev
-- A `nothing` run stays out of the Inbox: its session is marked seen.
ALTER TABLE routine_runs ADD COLUMN outcome TEXT
  CHECK (outcome IN ('did_work', 'nothing', 'failed', 'needs_person'));
ALTER TABLE routine_runs ADD COLUMN outcome_source TEXT
  CHECK (outcome_source IN ('exit', 'rule', 'jev'));
INSERT OR IGNORE INTO schema_version (version) VALUES (133);
