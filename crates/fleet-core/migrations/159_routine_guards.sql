-- Automation guards (Orbit Fleet M15 step G3.8): what keeps a routine from
-- running away or failing quietly. The rules are in `service::routines`.
--   time_zone      the IANA zone the editor saved it in (`Europe/Bratislava`),
--                  shown beside the schedule; `utc_offset_min` still drives
--                  the schedule. NULL = not recorded.
--   run_max_secs   a run still going after this long is stopped (Escape in
--                  its pane) and failed; NULL = no time cap.
--   fallback_host  a run starts here when `host_alias` is unreachable, its
--                  login is past `accounts.pause_at`, or its start failed.
--                  Same org as `host_alias`. NULL = none.
--   retry_once     1: a failed run is started again once, on the next pass.
--   autonomy       0 report only | 1 ask before push | 2 push and open pull
--                  requests (the same as NULL): a line added to the run's
--                  prompt. Sessions run without permission prompts, so it is
--                  what the agent is told, not a sandbox.
ALTER TABLE routines ADD COLUMN time_zone TEXT;
ALTER TABLE routines ADD COLUMN run_max_secs INTEGER;
ALTER TABLE routines ADD COLUMN fallback_host TEXT;
ALTER TABLE routines ADD COLUMN retry_once INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routines ADD COLUMN autonomy INTEGER CHECK (autonomy BETWEEN 0 AND 2);

-- routine_runs
--   error_code     why it failed, as a code (`E_SSH`, `E_RUN_TIME_CAP`, …): the
--                  start's IpcError code, or the scheduler's own for a close.
--                  `service::routines::fix` names the fix from it.
--   host_alias     the host its session started on: the routine's, or its
--                  fallback. NULL for a run from before 159 or none started.
ALTER TABLE routine_runs ADD COLUMN error_code TEXT;
ALTER TABLE routine_runs ADD COLUMN host_alias TEXT;
-- The fleet's spend today (`automation.daily_budget`) sums every routine's
-- runs since a time.
CREATE INDEX IF NOT EXISTS routine_runs_started ON routine_runs (started_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (159);
