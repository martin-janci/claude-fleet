-- Every addressable endpoint in the fleet gets a durable identity here, so an
-- address can be resolved to something that survives a session move. A move
-- creates a NEW sessions row on the target and kills the source
-- (service/move_session/finalise.rs), so a message pointed at a session id
-- would be orphaned (and, before this migration, DELETEd outright by
-- store/sessions.rs delete_session). Messages point at a participant; a move
-- re-points the participant.
--
-- kind: 'session' (session_id set), 'client' (client_id -> client_tokens),
--       'hub' (both NULL; one row per fleet).
-- retired_at: tombstone. A retired participant resolves, and says it is gone,
--       instead of vanishing and leaving the sender with a silent timeout.
CREATE TABLE IF NOT EXISTS participants (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  session_id INTEGER,
  client_id INTEGER,
  created_at INTEGER NOT NULL,
  retired_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_participants_session
  ON participants(session_id) WHERE session_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_participants_client
  ON participants(client_id) WHERE client_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_participants_retired
  ON participants(retired_at) WHERE retired_at IS NOT NULL;

-- One participant per existing session, so no message is left unaddressable.
INSERT INTO participants (kind, session_id, created_at)
  SELECT 'session', id, strftime('%s','now') FROM sessions
  WHERE id NOT IN (SELECT session_id FROM participants WHERE session_id IS NOT NULL);

ALTER TABLE session_messages ADD COLUMN from_participant_id INTEGER;
ALTER TABLE session_messages ADD COLUMN to_participant_id INTEGER;
-- Handed to a hook response. Distinct from read_at: we know we wrote the body,
-- never that Claude ingested it (the turn may have been interrupted).
ALTER TABLE session_messages ADD COLUMN delivered_at INTEGER;

UPDATE session_messages SET
  from_participant_id = (SELECT id FROM participants WHERE session_id = from_session_id),
  to_participant_id   = (SELECT id FROM participants WHERE session_id = to_session_id);

CREATE INDEX IF NOT EXISTS idx_session_messages_undelivered
  ON session_messages(to_participant_id, delivered_at, sent_at);

-- Consecutive Stop-block count, so a remote sender can never shut a session
-- inside a never-ending turn (STOP_BLOCK_STREAK_MAX).
ALTER TABLE sessions ADD COLUMN stop_block_streak INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (43);
