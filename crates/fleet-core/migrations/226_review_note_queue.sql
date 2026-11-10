-- Review notes sent while their session was busy (docs/review-notes.md): the
-- prompt a send kept for the session's next idle moment, so `list` can say
-- the notes are still waiting, or never went out (the prompt was cancelled
-- in Send later, failed for good, or was dropped), rather than "Sent".
--
-- review_note_queue
--   session_id          the session of the send (moves with its notes)
--   batch               the send's client_msg_id (review_notes.batch)
--   deferred_prompt_id  the kept prompt (deferred_prompts.id); NULL when the
--                       send was typed at once but its typing may not have
--                       landed (see error)
--   error               why the send may not have reached the session
-- A send typed at once and confirmed has no row here.
CREATE TABLE IF NOT EXISTS review_note_queue (
  session_id         INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  batch              TEXT    NOT NULL,
  deferred_prompt_id INTEGER,
  error              TEXT,
  PRIMARY KEY (session_id, batch)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (226);
