-- Control's journal and its thread mutes (Control review F10, F12).
--
-- "The model decides, Fleet owns the state": what Control's agent would
-- otherwise keep only in its conversation, so a move, a restart, a
-- compaction or a new chat starts from it instead of from nothing.
--
-- `control_journal` holds what no other table already has: the last thing
-- the person said to Control (`said`, the first 200 characters, as a
-- conversation's `first_prompt`), a question Control asked the person and
-- is waiting on (`ask`, resolved when the person next speaks) and Control's
-- own notes (`note`). The open handoffs and the missions followed are read
-- live from `control_handoffs`, never copied. `Store::insert_journal_entry`
-- keeps the newest rows only, so the table cannot grow without bound.
--
-- `control_mutes` is a thread the person (or Control) muted in the Control
-- threads column: fleet stops telling Control about it until it is unmuted.
CREATE TABLE IF NOT EXISTS control_journal (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  at          INTEGER NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN ('said', 'ask', 'note')),
  text        TEXT    NOT NULL,
  resolved_at INTEGER
);

CREATE INDEX IF NOT EXISTS control_journal_by_kind ON control_journal (kind, id);

CREATE TABLE IF NOT EXISTS control_mutes (
  kind      TEXT    NOT NULL CHECK (kind IN ('session', 'mission')),
  target_id INTEGER NOT NULL,
  at        INTEGER NOT NULL,
  PRIMARY KEY (kind, target_id)
) WITHOUT ROWID;

INSERT OR IGNORE INTO schema_version (version) VALUES (198);
