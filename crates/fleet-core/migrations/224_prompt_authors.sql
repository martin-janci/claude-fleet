-- Who sent each prompt (HumanLayer plan v2, item 4): a hub with several
-- people shows a colleague's prompt as theirs, not as "You". One row per
-- prompt a hub tool typed or queued; the conversation matches a turn to
-- its row by the prompt's head and time (`service::transcript`).
--
-- prompt_authors
--   session_id        the session the prompt went to
--   at                when it was sent (or queued), unix seconds
--   head              the prompt's first characters, whitespace folded,
--                     as typed (an untrusted marker included)
--   author            who sent it as the conversation shows it
--                     ("Ada · phone"), and author_person_id their person
-- Rows go with their session; a session keeps its newest 500.
CREATE TABLE IF NOT EXISTS prompt_authors (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  at               INTEGER NOT NULL,
  head             TEXT    NOT NULL,
  author           TEXT    NOT NULL,
  author_person_id INTEGER
);

CREATE INDEX IF NOT EXISTS prompt_authors_session
  ON prompt_authors (session_id, at);

INSERT OR IGNORE INTO schema_version (version) VALUES (224);
