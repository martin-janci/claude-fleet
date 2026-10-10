-- Rules that say how a task's sessions start (Orbit Fleet M15 step G7.1):
-- a start rule names the host (with a fallback), the account, the model and
-- effort, and the agent; a placement rule (work_rules) names the host and
-- the account its tasks' sessions start on (those two columns are added in
-- Rust, see below). Where each decides is in `trackers::tickets::
-- plan_resolved` and `service::start_rules`. NULL everywhere = as before.
--
-- start_rules
--   fallback_host  where a start lands when `host_alias` is unreachable.
--                  NULL = none (the start waits on `host_alias`).
--   profile        the credential profile the session bills (the Account);
--                  NULL = the host's own login. Claude Code only.
--   model          `claude --model` value; NULL = the host's default.
--   effort         one of `validate::EFFORT_LEVELS`; NULL = the default.
--   agent          `claude` | `codex`; NULL = Claude Code.
ALTER TABLE start_rules ADD COLUMN fallback_host TEXT;
ALTER TABLE start_rules ADD COLUMN profile TEXT;
ALTER TABLE start_rules ADD COLUMN model TEXT;
ALTER TABLE start_rules ADD COLUMN effort TEXT;
ALTER TABLE start_rules ADD COLUMN agent TEXT;

-- work_rules gains host_alias ("its sessions start here") and profile (the
-- account those sessions bill) too, but not here: a database whose 066 was
-- skipped gets work_rules only from the repair that runs after every
-- migration, so `Store::ensure_work_rules_start_columns` adds them then.

INSERT OR IGNORE INTO schema_version (version) VALUES (161);
