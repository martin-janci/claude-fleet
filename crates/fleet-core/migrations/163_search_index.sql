-- Search phase 3: one full-text index over what a person looks for — tasks
-- and tickets, sessions, conversations, pull requests and the work journal
-- (and, only with `search.index_transcripts` on, conversation text the
-- transcript pass copies in; off by default, see `service/search_index.rs`).
--
-- search_docs  one row per searchable thing, kept in step with its source
--              by the triggers below, whatever code path writes the source.
--   kind       item | session | conversation | pr | journal | transcript
--   ref        the source row's id (a transcript chunk: `<claude id>:<offset>`)
--   session_id / claude_session_id / item_id
--              what the search's privacy fence checks a hit against: a
--              session row through `sees_session_row`, a conversation of a
--              gone session through `sees_past_conversation`, an item
--              through the org's visible items.
--   title, body  the text; `at` the time a hit is ordered by.
-- search_fts   FTS5 over search_docs (external content): case and accents
--              folded by `unicode61 remove_diacritics 2`, as `fold` does
--              for the in-memory searches.
--
-- Idempotent: tables and triggers `IF NOT EXISTS`, the backfill an upsert,
-- and the index rebuilt from search_docs at the end, so a re-run leaves
-- the same rows.
CREATE TABLE IF NOT EXISTS search_docs (
  id                INTEGER PRIMARY KEY,
  kind              TEXT    NOT NULL,
  ref               TEXT    NOT NULL,
  session_id        INTEGER,
  claude_session_id TEXT,
  item_id           INTEGER,
  title             TEXT    NOT NULL DEFAULT '',
  body              TEXT    NOT NULL DEFAULT '',
  at                INTEGER NOT NULL DEFAULT 0,
  UNIQUE (kind, ref)
);
CREATE INDEX IF NOT EXISTS idx_search_docs_session ON search_docs (session_id);
CREATE INDEX IF NOT EXISTS idx_search_docs_claude ON search_docs (claude_session_id);

CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5 (
  title,
  body,
  content = 'search_docs',
  content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2',
  prefix = '2 3'
);

-- search_docs → search_fts.
CREATE TRIGGER IF NOT EXISTS search_docs_ai AFTER INSERT ON search_docs BEGIN
  INSERT INTO search_fts (rowid, title, body) VALUES (new.id, new.title, new.body);
END;
CREATE TRIGGER IF NOT EXISTS search_docs_ad AFTER DELETE ON search_docs BEGIN
  INSERT INTO search_fts (search_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
END;
CREATE TRIGGER IF NOT EXISTS search_docs_au AFTER UPDATE ON search_docs BEGIN
  INSERT INTO search_fts (search_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
  INSERT INTO search_fts (rowid, title, body) VALUES (new.id, new.title, new.body);
END;

-- work_items → item: key and title; the brief and the tracker's description
-- excerpt (`meta.description`).
CREATE TRIGGER IF NOT EXISTS search_items_ai AFTER INSERT ON work_items BEGIN
  INSERT INTO search_docs (kind, ref, item_id, title, body, at)
  VALUES ('item', CAST(new.id AS TEXT), new.id,
          trim(coalesce(new.key, '') || ' ' || coalesce(new.title, '')),
          trim(coalesce(new.notes, '') || ' ' ||
               coalesce(CASE WHEN json_valid(new.meta) THEN json_extract(new.meta, '$.description') END, '')),
          new.updated_at)
  ON CONFLICT (kind, ref) DO UPDATE SET
    item_id = excluded.item_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
CREATE TRIGGER IF NOT EXISTS search_items_au AFTER UPDATE OF key, title, notes, meta ON work_items BEGIN
  INSERT INTO search_docs (kind, ref, item_id, title, body, at)
  VALUES ('item', CAST(new.id AS TEXT), new.id,
          trim(coalesce(new.key, '') || ' ' || coalesce(new.title, '')),
          trim(coalesce(new.notes, '') || ' ' ||
               coalesce(CASE WHEN json_valid(new.meta) THEN json_extract(new.meta, '$.description') END, '')),
          new.updated_at)
  ON CONFLICT (kind, ref) DO UPDATE SET
    item_id = excluded.item_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
CREATE TRIGGER IF NOT EXISTS search_items_ad AFTER DELETE ON work_items BEGIN
  DELETE FROM search_docs WHERE kind = 'item' AND ref = CAST(old.id AS TEXT);
END;

-- sessions → session: its names, host, branch, tags, last prompt and notes.
-- Only the text columns fire the update trigger: a status tick does not.
CREATE TRIGGER IF NOT EXISTS search_sessions_ai AFTER INSERT ON sessions BEGIN
  INSERT INTO search_docs (kind, ref, session_id, title, body, at)
  VALUES ('session', CAST(new.id AS TEXT), new.id,
          coalesce(nullif(new.friendly_name, ''), new.tmux_name),
          trim(new.tmux_name || ' ' || new.host_alias || ' ' || coalesce(new.worktree_key, '') || ' ' ||
               coalesce(new.tags, '') || ' ' || coalesce(new.last_prompt, '') || ' ' || coalesce(new.notes, '')),
          coalesce(new.last_activity_at, new.created_at, 0))
  ON CONFLICT (kind, ref) DO UPDATE SET
    session_id = excluded.session_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
CREATE TRIGGER IF NOT EXISTS search_sessions_au
AFTER UPDATE OF tmux_name, friendly_name, host_alias, worktree_key, tags, last_prompt, notes ON sessions BEGIN
  INSERT INTO search_docs (kind, ref, session_id, title, body, at)
  VALUES ('session', CAST(new.id AS TEXT), new.id,
          coalesce(nullif(new.friendly_name, ''), new.tmux_name),
          trim(new.tmux_name || ' ' || new.host_alias || ' ' || coalesce(new.worktree_key, '') || ' ' ||
               coalesce(new.tags, '') || ' ' || coalesce(new.last_prompt, '') || ' ' || coalesce(new.notes, '')),
          coalesce(new.last_activity_at, new.created_at, 0))
  ON CONFLICT (kind, ref) DO UPDATE SET
    session_id = excluded.session_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
-- A session's rows go with it (`sessions.id` is a reused rowid): its own
-- doc and its transcript chunks. A conversation's doc goes with the
-- conversation row (cascade); the journal keeps what outlives the session.
CREATE TRIGGER IF NOT EXISTS search_sessions_ad AFTER DELETE ON sessions BEGIN
  DELETE FROM search_docs WHERE kind IN ('session', 'transcript') AND session_id = old.id;
END;

-- conversations → conversation: the first prompt.
CREATE TRIGGER IF NOT EXISTS search_conversations_ai AFTER INSERT ON conversations
WHEN new.first_prompt IS NOT NULL AND new.first_prompt != '' BEGIN
  INSERT INTO search_docs (kind, ref, session_id, claude_session_id, title, body, at)
  VALUES ('conversation', CAST(new.id AS TEXT), new.session_id, new.claude_session_id, '', new.first_prompt, new.started_at)
  ON CONFLICT (kind, ref) DO UPDATE SET body = excluded.body;
END;
CREATE TRIGGER IF NOT EXISTS search_conversations_au AFTER UPDATE OF first_prompt ON conversations
WHEN new.first_prompt IS NOT NULL AND new.first_prompt != '' BEGIN
  INSERT INTO search_docs (kind, ref, session_id, claude_session_id, title, body, at)
  VALUES ('conversation', CAST(new.id AS TEXT), new.session_id, new.claude_session_id, '', new.first_prompt, new.started_at)
  ON CONFLICT (kind, ref) DO UPDATE SET body = excluded.body;
END;
CREATE TRIGGER IF NOT EXISTS search_conversations_ad AFTER DELETE ON conversations BEGIN
  DELETE FROM search_docs WHERE kind = 'conversation' AND ref = CAST(old.id AS TEXT);
END;

-- pull_requests → pr: its title, repo and branch.
CREATE TRIGGER IF NOT EXISTS search_prs_ai AFTER INSERT ON pull_requests BEGIN
  INSERT INTO search_docs (kind, ref, session_id, title, body, at)
  VALUES ('pr', CAST(new.id AS TEXT), new.session_id, coalesce(new.title, ''),
          trim(coalesce(new.repo, '') || ' ' || coalesce(new.head_ref, '') || ' ' || new.url), new.updated_at)
  ON CONFLICT (kind, ref) DO UPDATE SET
    session_id = excluded.session_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
CREATE TRIGGER IF NOT EXISTS search_prs_au AFTER UPDATE OF title, head_ref, session_id ON pull_requests BEGIN
  INSERT INTO search_docs (kind, ref, session_id, title, body, at)
  VALUES ('pr', CAST(new.id AS TEXT), new.session_id, coalesce(new.title, ''),
          trim(coalesce(new.repo, '') || ' ' || coalesce(new.head_ref, '') || ' ' || new.url), new.updated_at)
  ON CONFLICT (kind, ref) DO UPDATE SET
    session_id = excluded.session_id, title = excluded.title, body = excluded.body, at = excluded.at;
END;
CREATE TRIGGER IF NOT EXISTS search_prs_ad AFTER DELETE ON pull_requests BEGIN
  DELETE FROM search_docs WHERE kind = 'pr' AND ref = CAST(old.id AS TEXT);
END;

-- work_journal → journal: a conversation's end, a handover, a summary.
CREATE TRIGGER IF NOT EXISTS search_journal_ai AFTER INSERT ON work_journal
WHEN new.body IS NOT NULL AND new.body != '' BEGIN
  INSERT INTO search_docs (kind, ref, claude_session_id, title, body, at)
  VALUES ('journal', CAST(new.id AS TEXT), new.claude_session_id, new.kind, new.body, new.at)
  ON CONFLICT (kind, ref) DO UPDATE SET body = excluded.body;
END;
CREATE TRIGGER IF NOT EXISTS search_journal_au AFTER UPDATE OF body ON work_journal BEGIN
  DELETE FROM search_docs WHERE kind = 'journal' AND ref = CAST(new.id AS TEXT)
    AND (new.body IS NULL OR new.body = '');
  INSERT INTO search_docs (kind, ref, claude_session_id, title, body, at)
  SELECT 'journal', CAST(new.id AS TEXT), new.claude_session_id, new.kind, new.body, new.at
   WHERE new.body IS NOT NULL AND new.body != ''
  ON CONFLICT (kind, ref) DO UPDATE SET body = excluded.body;
END;
CREATE TRIGGER IF NOT EXISTS search_journal_ad AFTER DELETE ON work_journal BEGIN
  DELETE FROM search_docs WHERE kind = 'journal' AND ref = CAST(old.id AS TEXT);
END;

-- Backfill what is there already.
INSERT INTO search_docs (kind, ref, item_id, title, body, at)
SELECT 'item', CAST(id AS TEXT), id,
       trim(coalesce(key, '') || ' ' || coalesce(title, '')),
       trim(coalesce(notes, '') || ' ' ||
            coalesce(CASE WHEN json_valid(meta) THEN json_extract(meta, '$.description') END, '')),
       updated_at
  FROM work_items WHERE true
ON CONFLICT (kind, ref) DO NOTHING;
INSERT INTO search_docs (kind, ref, session_id, title, body, at)
SELECT 'session', CAST(id AS TEXT), id,
       coalesce(nullif(friendly_name, ''), tmux_name),
       trim(tmux_name || ' ' || host_alias || ' ' || coalesce(worktree_key, '') || ' ' ||
            coalesce(tags, '') || ' ' || coalesce(last_prompt, '') || ' ' || coalesce(notes, '')),
       coalesce(last_activity_at, created_at, 0)
  FROM sessions WHERE true
ON CONFLICT (kind, ref) DO NOTHING;
INSERT INTO search_docs (kind, ref, session_id, claude_session_id, title, body, at)
SELECT 'conversation', CAST(id AS TEXT), session_id, claude_session_id, '', first_prompt, started_at
  FROM conversations WHERE first_prompt IS NOT NULL AND first_prompt != ''
ON CONFLICT (kind, ref) DO NOTHING;
INSERT INTO search_docs (kind, ref, session_id, title, body, at)
SELECT 'pr', CAST(id AS TEXT), session_id, coalesce(title, ''),
       trim(coalesce(repo, '') || ' ' || coalesce(head_ref, '') || ' ' || url), updated_at
  FROM pull_requests WHERE true
ON CONFLICT (kind, ref) DO NOTHING;
INSERT INTO search_docs (kind, ref, claude_session_id, title, body, at)
SELECT 'journal', CAST(id AS TEXT), claude_session_id, kind, body, at
  FROM work_journal WHERE body IS NOT NULL AND body != ''
ON CONFLICT (kind, ref) DO NOTHING;

-- Where the transcript pass is in each session's transcript: the file
-- (`source`, its name) and the bytes of it already indexed. Goes with the
-- session (a reused rowid must not inherit a cursor); dropped wholesale
-- when `search.index_transcripts` is turned off.
CREATE TABLE IF NOT EXISTS search_transcript_cursors (
  session_id   INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  source       TEXT    NOT NULL,
  offset_bytes INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

-- The index from search_docs, whatever the triggers above did on a re-run.
INSERT INTO search_fts (search_fts) VALUES ('rebuild');

INSERT OR IGNORE INTO schema_version (version) VALUES (163);
