-- PR shepherd (docs/superpowers/specs/2026-10-08-pr-shepherd-design.md):
-- the standing rule a person grants per project, and one row per thing the
-- shepherd saw wrong with a session's pull request.
--
-- pr_shepherd_rules: no row, no shepherd. A person writes it (the hub's
-- `fleet-hub shepherd grant`); nothing an agent can call writes it.
--   level        watch  record what is wrong, send nothing
--                nudge  also ask the session's own Claude to fix it
--                merge  also merge a green PR (the merge queue, a later
--                       step; until it lands, merge acts as nudge)
--   granted_by   who granted it, for the record ("console" for the CLI)
--   expires_at   unix seconds; NULL never expires. An expired rule is
--                treated as absent.
--   recipes      free text the project adds to a conflict prompt (its
--                REGEN_* commands, say), at most 2000 characters
CREATE TABLE IF NOT EXISTS pr_shepherd_rules (
  project_id  INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  level       TEXT NOT NULL CHECK (level IN ('watch', 'nudge', 'merge')),
  granted_by  TEXT NOT NULL,
  granted_at  INTEGER NOT NULL,
  expires_at  INTEGER,
  recipes     TEXT CHECK (recipes IS NULL OR length(recipes) <= 2000)
);

-- pr_shepherd_episodes: one problem on one pushed commit. The key is what
-- makes the shepherd act at most once per problem: a new push is a new
-- head_oid and so a new episode.
--   condition    conflict | behind | ci_red
--   outcome      watched | nudged | skipped:<why> | failed:<error>
CREATE TABLE IF NOT EXISTS pr_shepherd_episodes (
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  head_oid    TEXT NOT NULL,
  condition   TEXT NOT NULL CHECK (condition IN ('conflict', 'behind', 'ci_red')),
  pr_url      TEXT,
  at          INTEGER NOT NULL,
  outcome     TEXT NOT NULL,
  PRIMARY KEY (session_id, head_oid, condition)
);
CREATE INDEX IF NOT EXISTS pr_shepherd_episodes_at
  ON pr_shepherd_episodes (session_id, at);

INSERT OR IGNORE INTO schema_version (version) VALUES (128);
