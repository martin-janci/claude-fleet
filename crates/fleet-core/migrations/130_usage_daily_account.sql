-- Orbit Fleet redesign step 4.2, cost per account and per model:
-- usage_daily_account is the same daily roll-up as usage_daily, keyed by the
-- session's account (`sessions.account_uuid` at booking time: the host's
-- login or the session's profile login) and the model that spent it, booked
-- beside usage_daily in the same pass.
--   account_uuid  '' when the session has no known account yet
--   model         the transcript's model id; '' when a line had none
-- No foreign key on accounts: spend history outlives an account row.
-- It starts empty: spend from before this migration has no account or
-- model split.
CREATE TABLE IF NOT EXISTS usage_daily_account (
    day INTEGER NOT NULL,
    account_uuid TEXT NOT NULL,
    model TEXT NOT NULL,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, account_uuid, model, backfill)
);

CREATE INDEX IF NOT EXISTS usage_daily_account_by_account
    ON usage_daily_account (account_uuid, day);

INSERT OR IGNORE INTO schema_version (version) VALUES (130);
