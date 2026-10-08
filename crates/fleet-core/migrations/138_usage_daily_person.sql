-- 138: `usage_daily_person`, an org's daily spend split by the person whose
-- session it was (Orbit Fleet redesign step 11.8, docs/ux/2026-10-08-orbit-
-- fleet-redesign/transition-plan.md): the org page's "By person" table.
--
-- The same roll-up as usage_daily_org (106), booked beside it in the same
-- pass, keyed also by the session's `owner_person_id` at booking time.
-- `person_id` 0 is nobody: a routine's, a mission's or an unclaimed
-- session (0, not NULL, so the primary key holds). A session in no org
-- books nothing here. It starts empty: spend from before this migration has
-- no person, and the page says "since" the first row it has.
--
-- Who reads it follows the org spend rule, stricter: only a caller that sees
-- every session row there is, so the table is all or nothing
-- (service::org_spend).
CREATE TABLE IF NOT EXISTS usage_daily_person (
    day INTEGER NOT NULL,
    org_id INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    person_id INTEGER NOT NULL DEFAULT 0,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, org_id, person_id, backfill)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (138);
