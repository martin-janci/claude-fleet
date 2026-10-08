-- Orchestration O3 (docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md
-- §6): what a finished attempt left behind, and an item's acceptance
-- conditions.
--
-- `tasks.result_json`    the worker's own structured report (summary,
--                        outcome, tests_run, …), parsed from the block after
--                        its done marker; NULL when it printed only prose.
-- `tasks.evidence_json`  what git said about the attempt's checkout when it
--                        finished: commits and changed files against the
--                        base. Fleet reads it; the worker never supplies it.
-- `work_items.done_when` the item's condition lines, a JSON array typed by
--                        prefix (ci:<check>, review, test:<command>, person).
-- `work_item_verifications` a person's recorded checks of those lines; the
--                        newest row per (item, line) decides it.
ALTER TABLE tasks ADD COLUMN result_json TEXT;
ALTER TABLE tasks ADD COLUMN evidence_json TEXT;
ALTER TABLE work_items ADD COLUMN done_when TEXT;

CREATE TABLE IF NOT EXISTS work_item_verifications (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  line    TEXT    NOT NULL,
  ok      INTEGER NOT NULL,
  actor   TEXT    NOT NULL,
  note    TEXT,
  at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_work_item_verifications_line
  ON work_item_verifications(item_id, line, id DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (117);
