-- Jira-style ticket keys administered by fleet (`store::ticket_keys`): a
-- short upper-case prefix and a counter per "ticket space", so a ticket reads
-- `FLT-12` rather than `owner/repo#12` or `asana:1209876543210987`.
--
-- ticket_spaces  one numbering: a fleet project's (`scope` = `project:<id>`),
--                a GitHub repository no project claims (`github:<repo>`), or
--                an Asana tracker (`tracker:<id>`). `prefix` is unique
--                (case-insensitive); `former_prefixes` (JSON) keeps the ones a
--                rename replaced, so an old key still resolves.
-- work_items +=  `ticket_space_id` / `ticket_seq`: the number the item drew,
--                and `display_key` = prefix-seq, kept current on a rename.
--                A native item's `key` IS its display key; a tracker item
--                whose tracker has no human keys keeps its tracker key as
--                identity and answers to the display key too.
CREATE TABLE IF NOT EXISTS ticket_spaces (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  scope            TEXT    NOT NULL UNIQUE,
  prefix           TEXT    NOT NULL,
  next_seq         INTEGER NOT NULL DEFAULT 1,
  former_prefixes  TEXT,
  created_at       INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_ticket_spaces_prefix
  ON ticket_spaces(prefix COLLATE NOCASE);

ALTER TABLE work_items ADD COLUMN ticket_space_id INTEGER REFERENCES ticket_spaces(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN ticket_seq INTEGER;
ALTER TABLE work_items ADD COLUMN display_key TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_ticket_seq
  ON work_items(ticket_space_id, ticket_seq) WHERE ticket_space_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_display_key ON work_items(display_key);

INSERT OR IGNORE INTO schema_version (version) VALUES (169);
