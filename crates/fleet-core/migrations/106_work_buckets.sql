-- Sprints and releases in the native work graph (design 2026-09-28 §1).
--
-- One table for both: they share their whole shape and `kind` discriminates.
-- Fleet owns these (E1): a tracker's sprint or version is linked through
-- `work_bucket_refs` and may place synced items into a bucket ("adoption"),
-- but never renames, dates or closes it.
--
-- `state`: a sprint is planned | active | closed, a release planned |
-- released. `ends_at` is a sprint's end and a release's target date.
-- `shipped_ref` is typed in by a person and read by nothing automatically
-- (E3). `version` is the expected_version guard, as on work_placements.
CREATE TABLE IF NOT EXISTS work_buckets (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT    NOT NULL,
  name        TEXT    NOT NULL,
  org_id      INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  state       TEXT    NOT NULL,
  starts_at   INTEGER,
  ends_at     INTEGER,
  shipped_at  INTEGER,
  shipped_ref TEXT,
  goal        TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  version     INTEGER NOT NULL DEFAULT 1
);
-- Two orgs may both have "Sprint 24"; one org may not have it twice.
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_buckets_name
  ON work_buckets(kind, COALESCE(org_id, 0), name);

-- Membership, N:M with history. `removed_at` rather than a delete, so "this
-- did not finish in sprint 23" survives a close. `source` is manual (a
-- person) or adopted (a tracker reported it): adoption is withdrawn when the
-- tracker stops reporting it, and never touches a manual row. At most one
-- CURRENT sprint per item is enforced in `store::work_buckets`, where the
-- refusal can say which sprint holds it.
CREATE TABLE IF NOT EXISTS work_bucket_items (
  bucket_id  INTEGER NOT NULL REFERENCES work_buckets(id) ON DELETE CASCADE,
  item_id    INTEGER NOT NULL REFERENCES work_items(id)   ON DELETE CASCADE,
  source     TEXT    NOT NULL,
  added_at   INTEGER NOT NULL,
  removed_at INTEGER,
  PRIMARY KEY (bucket_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_work_bucket_items_item
  ON work_bucket_items(item_id) WHERE removed_at IS NULL;

-- A native bucket linked to a tracker's sprint or version. `external_id` is
-- what the tracker's snapshot reports for it (the sprint or version name
-- today); the bucket keeps its own name, dates and state.
CREATE TABLE IF NOT EXISTS work_bucket_refs (
  bucket_id     INTEGER NOT NULL REFERENCES work_buckets(id) ON DELETE CASCADE,
  tracker_id    INTEGER NOT NULL REFERENCES trackers(id)     ON DELETE CASCADE,
  external_id   TEXT    NOT NULL,
  external_name TEXT,
  last_seen_at  INTEGER,
  PRIMARY KEY (bucket_id, tracker_id, external_id)
);
CREATE INDEX IF NOT EXISTS idx_work_bucket_refs_tracker
  ON work_bucket_refs(tracker_id, external_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (106);
