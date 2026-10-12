-- Send later's time choices (Orbit Fleet M15 step G1.8): a deferred prompt
-- (migration 133) may also wait for a time, for its account's usage limit to
-- reset, and be dropped when its session is archived first
-- (`service::sessions::deferred`).
--   not_before        unix secs before which it is not typed (NULL: as soon
--                     as the session is idle)
--   until_limit_reset 1: not typed while the session's account is at or
--                     past `accounts.pause_at`
--   skip_if_archived  1: dropped, not typed, once the session is archived
--   skipped_at        when it was dropped for that reason
-- A pending row has `skipped_at` NULL as well; the partial index of 133 is
-- rebuilt to say so.
ALTER TABLE deferred_prompts ADD COLUMN not_before INTEGER;
ALTER TABLE deferred_prompts ADD COLUMN until_limit_reset INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deferred_prompts ADD COLUMN skip_if_archived INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deferred_prompts ADD COLUMN skipped_at INTEGER;

DROP INDEX IF EXISTS deferred_prompts_pending;
CREATE INDEX IF NOT EXISTS deferred_prompts_pending
  ON deferred_prompts (session_id, id)
  WHERE delivered_at IS NULL AND failed_at IS NULL AND cancelled_at IS NULL
    AND skipped_at IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (155);
