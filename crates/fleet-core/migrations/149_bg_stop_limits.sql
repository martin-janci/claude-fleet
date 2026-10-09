-- Redesign (phone "Background agent" sheet): a background agent may be
-- launched with "stop after" limits — a wall-clock deadline and/or a spend
-- cap. `claude --bg` has no such flag (`--max-budget-usd` works only with
-- `--print`), so the hub records the limits here and its reconcile tick
-- stops the agent (`claude stop`) once either is reached.
--
-- Keyed by the agent's Claude session id on its host, the identity its
-- `bg:<id>` fleet row is reconciled under; a row is written right after the
-- launch, before reconcile has matched the agent.
--
-- stop_at           unix secs after which the agent is stopped (NULL: none)
-- stop_cost_micros  spend cap in millionths of a USD, compared against the
--                   row's estimated `sessions.usage_cost_micros` (NULL: none)
-- stopped_at        when the tick stopped it, or found it already gone;
--                   NULL while the limit is live
-- reason            which limit fired: 'time' | 'cost' | 'gone'
CREATE TABLE IF NOT EXISTS bg_stop_limits (
    host_alias TEXT NOT NULL,
    claude_session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    stop_at INTEGER,
    stop_cost_micros INTEGER,
    stopped_at INTEGER,
    reason TEXT,
    PRIMARY KEY (host_alias, claude_session_id)
);
CREATE INDEX IF NOT EXISTS bg_stop_limits_live
    ON bg_stop_limits (stopped_at) WHERE stopped_at IS NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (149);
