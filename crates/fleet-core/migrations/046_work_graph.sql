-- Work graph, slice M1b (docs/superpowers/2026-09-24-work-graph-roadmap.md,
-- design §0.2): what a session is working on, durably.
--
-- work_items   a unit of work fleet knows by more than a key. Today only
--              `source='local'` items the user names; tracker items (Jira,
--              GitHub, Asana …) arrive with M3 and use the same table
--              (`tracker_id` + `external_id`; no FK yet, `trackers` does not
--              exist). `key` is the normalised human key when there is one.
-- work_links   session ↔ work, N:M. Anchored on the session's PARTICIPANT
--              (migration 043/045), never on the reusable `sessions.id`: a
--              move re-points the participant, so the link follows the
--              session to its new row for free. A link names either an item
--              or a bare `ref_key` (a key no item exists for yet — a later
--              tracker sync binds it). `state`: confirmed | rejected (a
--              rejection is sticky: detection must not re-propose it);
--              `suggested` arrives with detection (M4). `source` says why
--              the link exists (manual | started | agent | branch | …).
--              `ended_at` + `snap_*`: when the session's participant is
--              retired (kill, reap, host/project removal) the link is NOT
--              deleted — it ends, and keeps enough of the session to show it
--              as past work and to resume it (host, tmux name, label,
--              project, worktree, branch, PR, every Claude conversation id).
--
-- The snapshot is written by a trigger on the participant's retirement, the
-- same reasoning as 044/045: five raw-SQL sites retire participants, every
-- one of them BEFORE it deletes the sessions row, so the row (and its
-- conversations, which cascade with it) is still there to copy from. A
-- snapshot taken at link time would be stale after a move or a /clear.
CREATE TABLE IF NOT EXISTS work_items (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  source          TEXT    NOT NULL,
  tracker_id      INTEGER,
  external_id     TEXT,
  key             TEXT,
  title           TEXT    NOT NULL DEFAULT '',
  url             TEXT,
  status_category TEXT    NOT NULL DEFAULT 'todo',
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_local_key
  ON work_items(key) WHERE source = 'local' AND key IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_external
  ON work_items(tracker_id, external_id) WHERE external_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS work_links (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id          INTEGER REFERENCES work_items(id) ON DELETE CASCADE,
  ref_key          TEXT,
  participant_id   INTEGER REFERENCES participants(id) ON DELETE SET NULL,
  state            TEXT    NOT NULL,
  source           TEXT    NOT NULL,
  is_primary       INTEGER NOT NULL DEFAULT 0,
  created_at       INTEGER NOT NULL,
  decided_at       INTEGER,
  ended_at         INTEGER,
  snap_host        TEXT,
  snap_tmux        TEXT,
  snap_name        TEXT,
  snap_project_id  INTEGER,
  snap_worktree    TEXT,
  snap_branch      TEXT,
  snap_pr_url      TEXT,
  snap_claude_ids  TEXT,
  CHECK (item_id IS NOT NULL OR ref_key IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_work_links_live
  ON work_links(participant_id) WHERE ended_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_work_links_ref ON work_links(ref_key);
CREATE INDEX IF NOT EXISTS idx_work_links_item ON work_links(item_id);

CREATE TRIGGER IF NOT EXISTS trg_work_links_end_on_retire
AFTER UPDATE OF retired_at ON participants
WHEN OLD.retired_at IS NULL AND NEW.retired_at IS NOT NULL AND OLD.session_id IS NOT NULL
BEGIN
  UPDATE work_links SET
    ended_at        = NEW.retired_at,
    is_primary      = 0,
    snap_host       = COALESCE((SELECT host_alias     FROM sessions WHERE id = OLD.session_id), snap_host),
    snap_tmux       = COALESCE((SELECT tmux_name      FROM sessions WHERE id = OLD.session_id), snap_tmux),
    snap_name       = COALESCE((SELECT friendly_name  FROM sessions WHERE id = OLD.session_id), snap_name),
    snap_project_id = COALESCE((SELECT project_id     FROM sessions WHERE id = OLD.session_id), snap_project_id),
    snap_worktree   = COALESCE((SELECT worktree_key   FROM sessions WHERE id = OLD.session_id), snap_worktree),
    snap_branch     = COALESCE((SELECT w.branch FROM sessions s JOIN worktrees w ON w.id = s.worktree_id
                                 WHERE s.id = OLD.session_id), snap_branch),
    snap_pr_url     = COALESCE((SELECT pr_url         FROM sessions WHERE id = OLD.session_id), snap_pr_url),
    snap_claude_ids = COALESCE((SELECT json_group_array(claude_session_id) FROM
                                  (SELECT claude_session_id FROM conversations
                                    WHERE session_id = OLD.session_id ORDER BY started_at)
                                 HAVING COUNT(*) > 0), snap_claude_ids)
  WHERE participant_id = OLD.id AND ended_at IS NULL;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (46);
