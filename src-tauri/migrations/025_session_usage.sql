-- Wave 5 G1 (PROD-7): per-session token usage and estimated cost, collected
-- incrementally from Claude Code's JSONL transcript on the session's host.
--
-- sessions.usage_input_tokens        running sum of message.usage.input_tokens
-- sessions.usage_output_tokens       ... output_tokens
-- sessions.usage_cache_write_tokens  ... cache_creation_input_tokens
-- sessions.usage_cache_read_tokens   ... cache_read_input_tokens
-- sessions.usage_cost_micros         estimated cost in millionths of a USD,
--                                    priced per model when each delta lands
--                                    (service::usage price table, overridable
--                                    with the usage.prices_json setting)
-- sessions.usage_model               model of the most recent counted message
-- sessions.usage_offset_bytes        bytes of the transcript already summed;
--                                    the next pass reads from here
-- sessions.usage_updated_at          unix secs the totals last changed
-- sessions.usage_source              transcript file name (<uuid>.jsonl) the
--                                    offset refers to; a different file (e.g.
--                                    after /clear) restarts the offset at 0
--                                    and keeps accumulating
-- sessions.usage_last_msg_id         id of the last counted assistant
--                                    message: Claude writes one line per
--                                    content block, each repeating the same
--                                    usage, so a pass that starts mid-message
--                                    must not count it again
-- sessions.usage_last_msg_usage      what was counted for that message,
--                                    `in,out,cache_write,cache_read,cache_write_5m`:
--                                    newer Claude versions stream a message's
--                                    block lines with GROWING usage (output
--                                    tokens), so a repeat adds only the growth
-- usage_daily                       per-host, per-UTC-day sum of every
--                                    delta, so fleet_health / usage_report
--                                    can report spend per day even after the
--                                    session rows are gone
ALTER TABLE sessions ADD COLUMN usage_input_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_output_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_cache_write_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_cache_read_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_cost_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_model TEXT;
ALTER TABLE sessions ADD COLUMN usage_offset_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN usage_updated_at INTEGER;
ALTER TABLE sessions ADD COLUMN usage_source TEXT;
ALTER TABLE sessions ADD COLUMN usage_last_msg_id TEXT;
ALTER TABLE sessions ADD COLUMN usage_last_msg_usage TEXT;

CREATE TABLE IF NOT EXISTS usage_daily (
    day INTEGER NOT NULL,            -- unix secs / 86400 (UTC day number)
    host_alias TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, host_alias)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (25);
