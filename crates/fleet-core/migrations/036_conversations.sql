-- Conversation tracking (spec 2026-09-18-conversation-events-design.md).
--
-- conversations            one row per Claude Code conversation a session
--                          has run. At most one open row (ended_at IS NULL)
--                          per session: the one whose claude_session_id
--                          equals sessions.claude_session_id.
-- sessions.tmux_pane_id    the pane id (%17) reconcile last saw; hooks carry
--                          $TMUX_PANE in X-Fleet-Pane and resolve by it.
-- sessions.awaiting_rebind_at  set by SessionEnd(clear|resume): the next
--                          SessionStart / UserPromptSubmit from an unknown id
--                          in the same cwd on the same host rebinds this row.
-- sessions.context_*       context size of the current conversation.
--                          context_source: transcript | hook | pane.
-- sessions.model           model of the current conversation.
-- session_events.claude_session_id  the conversation an event belongs to.
CREATE TABLE IF NOT EXISTS conversations (
  id                INTEGER PRIMARY KEY,
  session_id        INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  claude_session_id TEXT    NOT NULL,
  transcript_path   TEXT,
  started_at        INTEGER NOT NULL,
  ended_at          INTEGER,
  start_source      TEXT    NOT NULL,
  end_reason        TEXT,
  model             TEXT,
  first_prompt      TEXT,
  turns             INTEGER NOT NULL DEFAULT 0,
  compactions       INTEGER NOT NULL DEFAULT 0,
  last_compact_at   INTEGER,
  UNIQUE (session_id, claude_session_id)
);
CREATE INDEX IF NOT EXISTS conversations_by_session
  ON conversations(session_id, started_at DESC);

ALTER TABLE sessions ADD COLUMN tmux_pane_id TEXT;
ALTER TABLE sessions ADD COLUMN awaiting_rebind_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN context_window INTEGER;
ALTER TABLE sessions ADD COLUMN context_source TEXT;
ALTER TABLE sessions ADD COLUMN context_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_stale INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN model TEXT;
ALTER TABLE session_events ADD COLUMN claude_session_id TEXT;

INSERT OR IGNORE INTO conversations (session_id, claude_session_id, transcript_path,
                                     started_at, start_source)
  SELECT id, claude_session_id, transcript_path, created_at, 'unknown'
  FROM sessions WHERE claude_session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (36);
