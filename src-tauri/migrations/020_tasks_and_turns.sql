-- Wave 3 Track E: orchestration — completion signal, task objects, tags.
--
-- sessions.turn_seq          monotonically increasing count of completed
--                            turns (incremented by the Stop hook). A caller
--                            that records `turn_seq` before `send_prompt` can
--                            wait for `turn_seq > before` (wait_for_session).
-- sessions.last_stop_at      unix secs of the last Stop hook. Reconcile does
--                            not overwrite a hook-stamped claude_status that
--                            is newer than the pass's pane observation.
-- sessions.parent_session_id the requester session that dispatched the task
--                            this worker is running (NULL for top-level rows).
-- sessions.tags              JSON array of short labels (set_session_tags;
--                            list_sessions { tag } filters on it). NULL = [].
-- session_messages.reply_to  id of the message this one answers (NULL when
--                            not a reply).
-- tasks                      one row per dispatched unit of work. States:
--                            queued → running → done | failed | cancelled.
--                            `nonce` tags the FLEET_TASK_DONE_<nonce> marker
--                            the worker is asked to print; the Stop hook
--                            scans the worker's transcript / pane for it and
--                            stores the paragraph after it as `result`.
ALTER TABLE sessions ADD COLUMN turn_seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_stop_at INTEGER;
ALTER TABLE sessions ADD COLUMN parent_session_id INTEGER;
ALTER TABLE sessions ADD COLUMN tags TEXT;
-- transcript_path   the JSONL path Claude Code reports in every hook body
--                   (Stop / UserPromptSubmit); preferred over any path fleet
--                   derives from the cwd, which misses symlinked checkouts
--                   and Claude's truncation of long encoded directory names.
-- last_hook_at      unix secs of the last Stop OR UserPromptSubmit hook. A
--                   reconcile pass that STARTED at or before it keeps the
--                   hook-stamped claude_status instead of the pane guess.
ALTER TABLE sessions ADD COLUMN transcript_path TEXT;
ALTER TABLE sessions ADD COLUMN last_hook_at INTEGER;

ALTER TABLE session_messages ADD COLUMN reply_to INTEGER;

CREATE TABLE tasks (
  id INTEGER PRIMARY KEY,
  requester_session_id INTEGER,
  worker_session_id INTEGER,
  prompt TEXT,
  state TEXT NOT NULL,
  result TEXT,
  error TEXT,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER,
  nonce TEXT NOT NULL,
  -- The worker's claude_session_id when the task was dispatched; a later
  -- different id means the worker was recreated onto a fresh conversation
  -- that never saw the prompt, and the task is failed.
  worker_claude_session_id TEXT
);
CREATE INDEX idx_tasks_state ON tasks(state);
CREATE INDEX idx_tasks_worker ON tasks(worker_session_id, state);
CREATE INDEX idx_tasks_requester ON tasks(requester_session_id, created_at DESC);

INSERT OR IGNORE INTO schema_version (version) VALUES (20);
