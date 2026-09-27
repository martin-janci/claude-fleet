-- Work graph M14 (docs/superpowers/specs/2026-09-27-work-view-design.md):
-- the Work view — versions for concurrent edits, local placement, placement
-- rules, saved views, a local item's own org and org-bound paired clients.
-- Additive only: every column has a default, every table is new.
--
-- work_links.version        bumped by the trigger below on every change a
--                           person can race (state, source, primary, end,
--                           target, archive, review ack): the compare-and-set
--                           token of `work_link { expected_version }`.
-- work_links.review_ack_at  a person kept a conflict on purpose (a forced
--                           cross-org link, a link to an unavailable ticket):
--                           the review inbox stops listing it.
-- work_items.org_id         a LOCAL item's own org, set by a person with an
--                           impact preview (`work_link { assign_org }`). A
--                           tracker item's org stays its tracker's; this
--                           column is never read for one.
-- work_placements           where a person put a task in the Work view (its
--                           group), keyed by the task id (`item:<id>` or
--                           `ref:<KEY>`, never a title). Navigation only:
--                           never a boundary.
-- work_rules                placement rules for similar tasks. Text-keyed
--                           conditions (review C22: never a project row id).
-- work_views                saved Work view filters; `owner_org` is the org
--                           of the org-bound client that saved it (NULL: an
--                           unrestricted caller), which is all such a client
--                           lists.
-- client_tokens.org_id      a paired client bound to one org (`fleet-hub pair
--                           --org`). Deliberately NO foreign key: deleting the
--                           org must not widen the client to every org — it
--                           stays bound to an id nothing has (fail closed).
ALTER TABLE work_links ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE work_links ADD COLUMN review_ack_at INTEGER;
ALTER TABLE work_items ADD COLUMN org_id INTEGER REFERENCES orgs(id) ON DELETE SET NULL;
ALTER TABLE client_tokens ADD COLUMN org_id INTEGER;

-- A client's org is part of who it is: re-binding it must invalidate every
-- cached caller (migration 060's epoch), exactly as a mode change does.
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_org
  AFTER UPDATE OF org_id ON client_tokens
  WHEN OLD.org_id IS NOT NEW.org_id
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

-- Only a real change bumps: a statement that rewrites a column with the
-- value it had (clearing `is_primary` on every sibling of a new primary)
-- must not turn another device's pending decision into a conflict.
CREATE TRIGGER IF NOT EXISTS trg_work_links_version
AFTER UPDATE OF state, source, is_primary, ended_at, item_id, ref_key, archived_at, review_ack_at
ON work_links
WHEN NEW.version = OLD.version
 AND (OLD.state IS NOT NEW.state OR OLD.source IS NOT NEW.source
      OR OLD.is_primary IS NOT NEW.is_primary OR OLD.ended_at IS NOT NEW.ended_at
      OR OLD.item_id IS NOT NEW.item_id OR OLD.ref_key IS NOT NEW.ref_key
      OR OLD.archived_at IS NOT NEW.archived_at OR OLD.review_ack_at IS NOT NEW.review_ack_at)
BEGIN
  UPDATE work_links SET version = OLD.version + 1 WHERE id = NEW.id;
END;

CREATE TABLE IF NOT EXISTS work_placements (
  task_id     TEXT    PRIMARY KEY,
  group_label TEXT,
  note        TEXT,
  version     INTEGER NOT NULL DEFAULT 1,
  updated_at  INTEGER NOT NULL,
  updated_by  TEXT
);

CREATE TABLE IF NOT EXISTS work_rules (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  name           TEXT    NOT NULL,
  enabled        INTEGER NOT NULL DEFAULT 1,
  tracker_id     INTEGER,
  container      TEXT,
  key_prefix     TEXT,
  title_contains TEXT,
  repo           TEXT,
  group_label    TEXT    NOT NULL,
  version        INTEGER NOT NULL DEFAULT 1,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  CHECK (tracker_id IS NOT NULL OR container IS NOT NULL OR key_prefix IS NOT NULL
         OR title_contains IS NOT NULL OR repo IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS work_views (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT    NOT NULL,
  filters     TEXT    NOT NULL,
  owner_org   INTEGER,
  version     INTEGER NOT NULL DEFAULT 1,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_views_name
  ON work_views(COALESCE(owner_org, 0), name);

INSERT OR IGNORE INTO schema_version (version) VALUES (63);
