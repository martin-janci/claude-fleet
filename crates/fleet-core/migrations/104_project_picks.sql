-- The New session picker (docs/superpowers/specs/2026-10-05-project-picker-design.md):
-- a person's choices per project — pinned, visibility (hide | keep) and the
-- picker group. Keyed by owner/repo TEXT, never project_id: project rows are
-- deleted and re-created, their ids re-derived (review C22, see 050). No
-- foreign key to projects, by design: a choice outlives a re-scan.
CREATE TABLE IF NOT EXISTS project_picks (
  owner      TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  pinned     INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  vis        TEXT    CHECK (vis IS NULL OR vis IN ('hide', 'keep')),
  grp        TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, repo)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (104);
