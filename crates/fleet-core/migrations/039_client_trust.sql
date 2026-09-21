-- A paired client the operator vouches for. NULL (the default, and every
-- row that predates this column) means the client's prompts reach an agent
-- behind the untrusted-content marker line, as they always have. Set — the
-- unix time the operator granted it — means the client's words are the
-- operator's own: `send_prompt` / `broadcast_prompt` / `send_message` from
-- it are delivered unmarked, exactly as the master token's `raw: true`. The
-- audit row still names the client. Granted at pairing (`pair_client
-- { trusted: true }`) or afterwards (`set_client_trust`), and taken back
-- the same way; a hub older than this column keeps marking everything,
-- which is the safe direction.
ALTER TABLE client_tokens ADD COLUMN trusted_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (39);
