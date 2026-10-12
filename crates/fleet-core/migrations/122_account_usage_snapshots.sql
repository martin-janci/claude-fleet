-- Account usage history (redesign step 2.5): one row per successful usage
-- fetch, so the 5-hour and weekly meters survive a restart and the Accounts
-- page (step 4.1) can draw their history and reset times.
-- *_pct       percent USED, 0..100; NULL when the endpoint left the bucket out
-- *_resets_at unix seconds; NULL when absent
-- source_host the host whose login answered
-- Rows older than the retention window are pruned on insert
-- (`store::account_usage_snapshots`).
CREATE TABLE IF NOT EXISTS account_usage_snapshots (
  id                          INTEGER PRIMARY KEY AUTOINCREMENT,
  account_uuid                TEXT    NOT NULL,
  fetched_at                  INTEGER NOT NULL,
  five_hour_pct               REAL,
  five_hour_resets_at         INTEGER,
  seven_day_pct               REAL,
  seven_day_resets_at         INTEGER,
  seven_day_opus_pct          REAL,
  seven_day_opus_resets_at    INTEGER,
  seven_day_sonnet_pct        REAL,
  seven_day_sonnet_resets_at  INTEGER,
  subscription                TEXT,
  source_host                 TEXT
);

CREATE INDEX IF NOT EXISTS account_usage_snapshots_by_account
  ON account_usage_snapshots (account_uuid, fetched_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (122);
