-- Assets S1b+S2 (M4): changeset cards, their items, and triage verdicts.
-- The DDL is the spec's verbatim (docs/superpowers/specs/
-- 2026-09-30-assets-s1b-s2-design.md, Data model). Vocabularies live in
-- `service::catalog::changesets`. CREATE IF NOT EXISTS: safe to re-run.
CREATE TABLE IF NOT EXISTS changesets (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL,        -- bootstrap | new | drift | rollout
  summary     TEXT NOT NULL,        -- the card's sentence
  state       TEXT NOT NULL,        -- proposed | applied | undone | dismissed | failed
  created_at  INTEGER NOT NULL,     -- Unix seconds
  applied_at  INTEGER,              -- Unix MILLISECONDS (latest applied = max(applied_at, id))
  commits     TEXT,                 -- JSON {catalog_id: sha}
  layers_snapshot TEXT,             -- JSON host_layers rows before apply
  error       TEXT
);

CREATE TABLE IF NOT EXISTS changeset_items (
  changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
  position     INTEGER NOT NULL,
  grp          TEXT    NOT NULL,    -- the card group (a layer name, "needs a look", ...)
  catalog_id   INTEGER REFERENCES catalogs(id),
  kind         TEXT NOT NULL,
  name         TEXT NOT NULL,
  action       TEXT NOT NULL,       -- import | assign_layer | set_scope | hide | take_host | restore | sync
  params       TEXT,                -- JSON
  decider      TEXT NOT NULL,       -- rule | jev | haiku | person
  state        TEXT NOT NULL,       -- pending | applied | skipped | rejected
  PRIMARY KEY (changeset_id, position)
);

CREATE TABLE IF NOT EXISTS asset_triage_verdicts (
  catalog_id   INTEGER REFERENCES catalogs(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,
  name         TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  verdict      TEXT NOT NULL,       -- ignored | rejected | host_local
  decider      TEXT NOT NULL,
  decided_at   INTEGER NOT NULL,
  PRIMARY KEY (kind, name, content_hash)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (94);
