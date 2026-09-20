-- User-set short display name for an account (the Hosts view design:
-- `m-janci@users.noreply.github.com` and `mj-janci@users.noreply.github.com` are easy to confuse in narrow
-- labels). This is user data: `upsert_account` (the probe path) must never
-- overwrite it — only `Store::set_account_nickname` does.
ALTER TABLE accounts ADD COLUMN nickname TEXT;

-- Whether `oauthAccount.hasExtraUsageEnabled` was true on the last probe:
-- hitting a usage limit spends pay-as-you-go money instead of blocking the
-- account, which changes the wording shown for the limit state.
ALTER TABLE accounts ADD COLUMN has_extra_usage INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (28);
