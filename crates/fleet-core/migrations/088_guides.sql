-- Declarative pages, guides (layout L9 `guide`): a guide stored at runtime.
-- An agent on a host proposes one over the control API (`guide { propose }`);
-- it is among the pages only once a person approves it.
--
-- One row per proposal. `spec` is the `fleet.page/1` JSON as validated when
-- it was proposed; `page_id` its id (always `guide.<name>`). At most one row
-- per page_id is `approved`: approving another supersedes it, and a newer
-- pending proposal for the same id supersedes the pending one.
--
-- state   'pending' | 'approved' | 'rejected' | 'superseded' | 'removed'
-- source  'agent' (a host or the control API) | 'person'
CREATE TABLE IF NOT EXISTS guide_proposals (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at            INTEGER NOT NULL,
  page_id       TEXT    NOT NULL,
  title         TEXT    NOT NULL,
  spec          TEXT    NOT NULL,
  why           TEXT,
  source        TEXT    NOT NULL,
  source_detail TEXT,
  state         TEXT    NOT NULL DEFAULT 'pending'
                CHECK (state IN ('pending', 'approved', 'rejected', 'superseded', 'removed')),
  decided_at    INTEGER,
  decided_by    TEXT
);
CREATE INDEX IF NOT EXISTS idx_guide_proposals_state
  ON guide_proposals(state, page_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (88);
