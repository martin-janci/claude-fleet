-- AI check on done (docs/work-graph.md#ai-check-on-done): a project's
-- switch for the AI tester that checks a ticket's finished work, and the
-- test command it runs.
--
-- projects
--   check_mode     off | ask | auto. NULL is off: a project tests nothing
--                  until a person turns it on. `ask` offers Test now on the
--                  task; `auto` starts the check when a ticket's work is
--                  finished (Finish task, or a run that ends done).
--   check_command  one plain command the check runs in the checkout
--                  (`cargo test`, `pnpm test`), checked by
--                  `orchestrate::checks::plain_command`; NULL runs none and
--                  the check only reviews the change against the ticket.
--                  Read apart from ProjectRow (`Store::project_check`).
--
-- ADD COLUMN is not idempotent: guarded in schema.rs.
ALTER TABLE projects ADD COLUMN check_mode TEXT;
ALTER TABLE projects ADD COLUMN check_command TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (213);
