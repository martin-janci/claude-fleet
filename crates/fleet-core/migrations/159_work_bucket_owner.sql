-- Personal sprints and releases (owner decision 2026-10-10: "in your private
-- space you may have your own sprints"). A bucket with an owner is that
-- person's alone: only they read it, plan into it and change it. A bucket
-- with none is the team's, as before.
--
-- `owner_person_id` has no foreign key, the migration-066 rationale: a
-- removed person must leave a row pointing at an id nothing has (fail
-- closed), never cascade into a widening.
--
-- The name is unique per kind, org AND owner, so two people may each keep a
-- "Sprint 1" of their own beside the team's.
-- ADD COLUMN is not idempotent: the migration is guarded on it.
ALTER TABLE work_buckets ADD COLUMN owner_person_id INTEGER;
DROP INDEX IF EXISTS ux_work_buckets_name;
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_buckets_name
  ON work_buckets(kind, COALESCE(org_id, 0), COALESCE(owner_person_id, 0), name);
CREATE INDEX IF NOT EXISTS idx_work_buckets_owner ON work_buckets(owner_person_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (159);
