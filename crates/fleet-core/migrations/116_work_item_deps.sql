-- Orchestration O2 (docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md
-- §4.3): the dependency graph between work items, and a person's hold.
--
-- One row says `item_id` waits for `depends_on`. `kind` is `blocks` (the
-- only kind yet). `source` says who drew the edge: person | planner |
-- proposal. Cycles are refused by the store in a transaction, not by a
-- trigger, and an edge joins two items of the same org only. READY and
-- BLOCKED are derived on read and never stored.
CREATE TABLE IF NOT EXISTS work_item_deps (
  item_id    INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  depends_on INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  kind       TEXT    NOT NULL DEFAULT 'blocks',
  source     TEXT    NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (item_id, depends_on),
  CHECK (item_id <> depends_on)
);
CREATE INDEX IF NOT EXISTS idx_work_item_deps_rev ON work_item_deps(depends_on);

-- A person stopped this item: it is never READY while `held_at` is set.
-- ADD COLUMN is not idempotent: the migration is guarded on it.
ALTER TABLE work_items ADD COLUMN held_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (116);
