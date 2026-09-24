-- Work graph M5 (docs/superpowers/plans/2026-09-24-work-graph-m5-orgs-and-isolation.md):
-- organisations and the org boundary.
--
-- orgs         a named organisation ("Company A"). Optional: a fleet with no
--              row here behaves exactly as before M5 (scopes are derived from
--              `projects.owner`, and there is no boundary). `isolate_sessions`
--              (decision D7, default off) also fences SESSIONS — list, peer
--              reads, messages, `session:*` frames — between this org and
--              every other, for per-host tokens; work data is always fenced.
-- org_rules    which sessions belong to an org, keyed by TEXT (review C22:
--              project rows are deleted and re-created, `project_id` is
--              re-derived on every pass, so a rule never names one). A rule
--              matches on every field it sets; the most specific match wins:
--              path > owner/repo > owner > host-only rule, then the host's
--              own `hosts.org_id`. `owner` is never `local` (an adopted
--              folder's placeholder owner means "no owner").
-- hosts.org_id the host's org: the BOUNDARY for its per-host token, and the
--              last fallback of a session's org. Set only by the master
--              (`work_admin assign_host`); a host can never move itself.
-- trackers.org_id (migration 048) gains its meaning here: a tracker item's
--              org is its tracker's.
-- work_links.snap_org_id
--              the session's org when the link ended, written by the trigger
--              below at participant retirement (the session row still exists
--              then, as 046's snapshot relies on), so past work keeps its org
--              after the session and its project row are gone.
CREATE TABLE IF NOT EXISTS orgs (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  name             TEXT    NOT NULL UNIQUE,
  color            TEXT,
  isolate_sessions INTEGER NOT NULL DEFAULT 0,
  created_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS org_rules (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id      INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  owner       TEXT,
  repo        TEXT,
  path_prefix TEXT,
  host_alias  TEXT,
  CHECK (owner IS NOT NULL OR path_prefix IS NOT NULL OR host_alias IS NOT NULL),
  CHECK (owner IS NULL OR owner <> 'local'),
  CHECK (repo IS NULL OR owner IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_org_rules_owner ON org_rules(owner);
CREATE INDEX IF NOT EXISTS idx_org_rules_org ON org_rules(org_id);

ALTER TABLE hosts ADD COLUMN org_id INTEGER REFERENCES orgs(id) ON DELETE SET NULL;
ALTER TABLE work_links ADD COLUMN snap_org_id INTEGER;

-- The session's org at retirement. The expression is the one
-- `store::orgs::SESSION_ORG_SQL` renders for `sessions.id = OLD.session_id`
-- (a test keeps the two equal). Independent of 046's snapshot trigger's
-- firing order: it fills links of this participant that have no org yet and
-- are live or ended by this very retirement.
CREATE TRIGGER IF NOT EXISTS trg_work_links_snap_org
AFTER UPDATE OF retired_at ON participants
WHEN OLD.retired_at IS NULL AND NEW.retired_at IS NOT NULL AND OLD.session_id IS NOT NULL
BEGIN
  UPDATE work_links SET snap_org_id = (
    SELECT COALESCE(
      (SELECT r.org_id FROM org_rules r
         LEFT JOIN projects op ON op.id = s.project_id
         LEFT JOIN worktrees ow ON ow.id = s.worktree_id
        WHERE (r.host_alias IS NULL OR r.host_alias = s.host_alias)
          AND (r.owner IS NULL OR (op.owner IS NOT NULL AND op.owner <> 'local'
                                   AND lower(op.owner) = lower(r.owner)))
          AND (r.repo IS NULL OR lower(op.repo) = lower(r.repo))
          AND (r.path_prefix IS NULL
               OR COALESCE(ow.path, op.base_path) = r.path_prefix
               OR substr(COALESCE(ow.path, op.base_path), 1, length(r.path_prefix) + 1)
                  = r.path_prefix || '/')
        ORDER BY CASE WHEN r.path_prefix IS NOT NULL THEN 3000 + length(r.path_prefix)
                      WHEN r.repo IS NOT NULL THEN 2000
                      WHEN r.owner IS NOT NULL THEN 1000
                      ELSE 0 END
                 + CASE WHEN r.host_alias IS NOT NULL THEN 1 ELSE 0 END DESC,
                 r.id ASC
        LIMIT 1),
      (SELECT h.org_id FROM hosts h WHERE h.alias = s.host_alias))
    FROM sessions s WHERE s.id = OLD.session_id)
  WHERE participant_id = OLD.id AND snap_org_id IS NULL
    AND (ended_at IS NULL OR ended_at = NEW.retired_at);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (50);
