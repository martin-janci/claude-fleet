-- Work graph M2 (docs/superpowers/plans/2026-09-24-work-graph-m2-resume-and-memory.md):
-- work memory, plus the carry columns on work_links.
--
-- work_journal   durable notes about Claude CONVERSATIONS, keyed by
--                `claude_session_id` — the join that survives a relink, a
--                reject, a move and a participant sweep: a link's
--                `snap_claude_ids` (or its live session's conversations)
--                names which journal rows belong to a work key, so history is
--                reassigned without being rewritten. Every fleet session is
--                journaled, linked or not, within per-conversation caps
--                (enforced by `Store::append_journal`).
--                kind: conversation | progress | compact_summary | outcome |
--                      note | handover
--                source: hook | transcript | probe | agent | fleet
--                `claude_session_id` is NULL only for a `handover` row addressed
--                to a session (its `participant_id`) whose conversation has not
--                started yet; `delivered_at` is stamped when a hook response
--                carries it (the brief rides `additionalContext`, never a pane).
-- work_links.role       work | review | worker (inherited links, M2.2).
-- work_links.resumable  0 once `purge_project` removed the transcripts its
--                       conversations would resume from.
--
-- The single `conversation` row per conversation is written by TRIGGERS, the
-- same reasoning as 044–046: a conversation closes on several paths
-- (SessionEnd, kill, rebind) and `conversations` rows die with their session
-- through an FK cascade on five raw-SQL delete sites plus the move. A trigger
-- on the close and one BEFORE the session delete catch all of them, before
-- the cascade.
CREATE TABLE IF NOT EXISTS work_journal (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  claude_session_id TEXT,
  participant_id    INTEGER REFERENCES participants(id) ON DELETE SET NULL,
  at                INTEGER NOT NULL,
  kind              TEXT    NOT NULL,
  source            TEXT    NOT NULL,
  body              TEXT,
  meta              TEXT,
  delivered_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_work_journal_conv
  ON work_journal(claude_session_id, kind, at DESC);
CREATE INDEX IF NOT EXISTS idx_work_journal_at ON work_journal(at);
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_journal_conversation
  ON work_journal(claude_session_id) WHERE kind = 'conversation';
CREATE INDEX IF NOT EXISTS idx_work_journal_handover
  ON work_journal(participant_id) WHERE kind = 'handover' AND delivered_at IS NULL;

ALTER TABLE work_links ADD COLUMN role TEXT NOT NULL DEFAULT 'work';
ALTER TABLE work_links ADD COLUMN resumable INTEGER NOT NULL DEFAULT 1;

CREATE TRIGGER IF NOT EXISTS trg_work_journal_conversation_closed
AFTER UPDATE OF ended_at ON conversations
WHEN NEW.ended_at IS NOT NULL AND OLD.ended_at IS NULL
BEGIN
  INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, body, meta)
  SELECT NEW.claude_session_id,
         (SELECT id FROM participants WHERE session_id = s.id AND retired_at IS NULL),
         NEW.ended_at, 'conversation', 'fleet', NEW.first_prompt,
         json_object('turns', NEW.turns, 'compactions', NEW.compactions,
                     'started_at', NEW.started_at, 'ended_at', NEW.ended_at,
                     'end_reason', NEW.end_reason, 'start_source', NEW.start_source,
                     'model', NEW.model, 'host', s.host_alias, 'tmux', s.tmux_name,
                     'name', s.friendly_name, 'project_id', s.project_id,
                     'worktree', s.worktree_key,
                     'branch', (SELECT w.branch FROM worktrees w WHERE w.id = s.worktree_id))
  FROM sessions s WHERE s.id = NEW.session_id
  ON CONFLICT(claude_session_id) WHERE kind = 'conversation' DO UPDATE SET
    at             = excluded.at,
    body           = COALESCE(excluded.body, work_journal.body),
    meta           = excluded.meta,
    participant_id = COALESCE(work_journal.participant_id, excluded.participant_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_work_journal_session_delete
BEFORE DELETE ON sessions
BEGIN
  INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, body, meta)
  SELECT c.claude_session_id,
         (SELECT id FROM participants WHERE session_id = OLD.id AND retired_at IS NULL),
         COALESCE(c.ended_at, CAST(strftime('%s', 'now') AS INTEGER)),
         'conversation', 'fleet', c.first_prompt,
         json_object('turns', c.turns, 'compactions', c.compactions,
                     'started_at', c.started_at, 'ended_at', c.ended_at,
                     'end_reason', COALESCE(c.end_reason, 'session_deleted'),
                     'start_source', c.start_source,
                     'model', c.model, 'host', OLD.host_alias, 'tmux', OLD.tmux_name,
                     'name', OLD.friendly_name, 'project_id', OLD.project_id,
                     'worktree', OLD.worktree_key,
                     'branch', (SELECT w.branch FROM worktrees w WHERE w.id = OLD.worktree_id))
  FROM conversations c WHERE c.session_id = OLD.id
  ON CONFLICT(claude_session_id) WHERE kind = 'conversation' DO UPDATE SET
    at             = excluded.at,
    body           = COALESCE(excluded.body, work_journal.body),
    meta           = excluded.meta,
    participant_id = COALESCE(work_journal.participant_id, excluded.participant_id);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (47);
