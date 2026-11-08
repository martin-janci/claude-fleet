-- Review notes on a session's changes (docs/review-notes.md): a person's
-- notes on lines of the diff, sent to the session's agent as one prompt
-- before anything is pushed. Drafts live on the device that writes them;
-- a note is stored once it is sent.
--
-- review_notes
--   session_id        the session whose changes the note is on
--   batch             the send's client_msg_id: a repeat of the same send
--                     finds its notes here and types nothing twice
--   seq               the note's number in its send (1-based), as the
--                     prompt numbers it
--   path, line, side  where: a path relative to the checkout, the line on
--                     the new (or, for a removed line, old) side; NULL line
--                     is a note on the whole file
--   quote             the line's text when the note was written
--   body              the note
--   author            who sent it as the conversation shows it
--                     ("Ada · desktop"), and author_person_id their person
--   created_at        when it was sent
--   resolved_at/by    when, and by whom, a person marked it resolved
CREATE TABLE IF NOT EXISTS review_notes (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  batch            TEXT    NOT NULL,
  seq              INTEGER NOT NULL,
  path             TEXT    NOT NULL,
  line             INTEGER,
  side             TEXT    NOT NULL DEFAULT 'new',
  quote            TEXT,
  body             TEXT    NOT NULL,
  author           TEXT    NOT NULL,
  author_person_id INTEGER,
  created_at       INTEGER NOT NULL,
  resolved_at      INTEGER,
  resolved_by      TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS review_notes_batch
  ON review_notes (session_id, batch, seq);

INSERT OR IGNORE INTO schema_version (version) VALUES (223);
