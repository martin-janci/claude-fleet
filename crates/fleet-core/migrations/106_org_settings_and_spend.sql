-- Org administration phase C
-- (docs/superpowers/specs/2026-10-06-org-administration-design.md).
--
-- org_settings: one org's own value for a registered setting that opts in
-- (`Spec::per_org`). No row means the org inherits the fleet's value. An
-- org's removal takes its values with it.
CREATE TABLE IF NOT EXISTS org_settings (
    org_id INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    set_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, key)
);

-- usage_daily_org: the same daily roll-up as usage_daily, keyed by the
-- session's org at booking time instead of its host, booked beside it in the
-- same pass. A session in no org books nothing here. It starts empty: spend
-- from before this migration has no org.
CREATE TABLE IF NOT EXISTS usage_daily_org (
    day INTEGER NOT NULL,
    org_id INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, org_id, backfill)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (106);
