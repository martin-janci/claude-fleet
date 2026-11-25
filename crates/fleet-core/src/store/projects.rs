//! Projects, worktrees and worktree parent fingerprints.

use super::*;
use std::collections::HashSet;

/// Ids of the sessions whose `worktree_id` is `worktree_id`: the rows a
/// re-point or clear is about to change, for the `session:updated` events
/// emitted after the commit.
fn session_ids_on_worktree(conn: &Connection, worktree_id: i64) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM sessions WHERE worktree_id=?1")?;
    let rows = stmt.query_map(rusqlite::params![worktree_id], |r| r.get(0))?;
    rows.collect()
}

/// Every `<table>.<column>` that holds a project id: what
/// [`Store::merge_project`] re-homes, and how. The test
/// `a_project_merge_covers_every_project_column` holds this list to the
/// schema, so a new table that names a project cannot be forgotten here.
pub(crate) const PROJECT_ID_COLUMNS: &[(&str, &str, ProjectRef)] = &[
    ("account_project_blocks", "project_id", ProjectRef::IntoWins),
    ("agent_rules", "project_id", ProjectRef::Moves),
    ("mission_lanes", "project_id", ProjectRef::Moves),
    (
        "orchestration_project_repos",
        "project_id",
        ProjectRef::IntoWins,
    ),
    ("pr_shepherd_merges", "project_id", ProjectRef::Moves),
    ("pr_shepherd_rules", "project_id", ProjectRef::IntoWins),
    ("pull_requests", "project_id", ProjectRef::Moves),
    ("routines", "project_id", ProjectRef::Moves),
    ("sessions", "project_id", ProjectRef::Moves),
    ("start_rules", "project_id", ProjectRef::IntoWins),
    ("work_items", "project_id", ProjectRef::Moves),
    ("work_links", "snap_project_id", ProjectRef::Moves),
    ("worktrees", "project_id", ProjectRef::Worktrees),
];

/// Project references the column scan of
/// `a_project_merge_covers_every_project_column` cannot see, because the
/// column is not named for a project: `routine_memory.owner_id` under
/// `scope = 'project'` (a project notebook, migration 204) and
/// `ticket_spaces.scope` = `project:<id>` (migration 169). The merge
/// re-homes both by hand, and the test holds this list to the schema; a new
/// reference of this kind belongs here and in `merge_project`.
#[cfg(test)]
pub(crate) const PROJECT_REFS_BY_OTHER_NAME: &[(&str, &str)] =
    &[("routine_memory", "owner_id"), ("ticket_spaces", "scope")];

/// Every `<table>.<column>` that points at a worktree row: what a merge
/// re-points at `into`'s row when it folds a twin worktree. Held to the
/// schema by the same test.
pub(crate) const WORKTREE_ID_COLUMNS: &[(&str, &str)] = &[
    ("mission_lanes", "worktree_id"),
    ("sessions", "worktree_id"),
];

/// How a project merge treats one project-id column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectRef {
    /// Every row moves.
    Moves,
    /// The column is part of a key: a row `into` already has keeps
    /// `into`'s and `from`'s goes; the rest move.
    IntoWins,
    /// Worktree rows, which sessions point at: see `merge_project`.
    Worktrees,
}

impl Store {
    pub fn get_project(&self, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
        fetch_project(&self.conn, id)
    }

    pub fn get_worktree_row(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {WORKTREE_COLUMNS} FROM worktrees WHERE id=?1"
            ))?
            .query_row(rusqlite::params![id], worktree_from_row)
            .optional()
    }

    // ---- Public mutation methods ----

    /// Upsert a project row scanned by `refresh_projects` or cloned by
    /// `service::add_project`: `adopted` is always cleared, since a scan or
    /// clone re-establishes the row's normal, rediscoverable shape even if it
    /// was previously adopted (e.g. the projects root grew to cover it).
    pub fn upsert_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.upsert_project_impl(owner, repo, base_path, false, false)
    }

    /// Upsert a project row registered by `service::add_project`'s `folder`
    /// source: `adopted` is set, so `refresh_projects`'s stale-rows sweep
    /// never deletes it merely for living outside the scanned root (its
    /// whole point) — see [`ProjectRow::adopted`].
    pub fn upsert_adopted_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.upsert_project_impl(owner, repo, base_path, true, false)
    }

    /// Upsert the project row that backs a fleet-internal working directory —
    /// today only the UX agent's (`service::operator`). `adopted` is left
    /// clear: this row was not registered from a folder by
    /// `service::add_project`, which is what `adopted` records. `system`
    /// alone is what `refresh_projects`'s stale-rows sweep must honour to
    /// leave it alone — see [`ProjectRow::system`].
    pub fn upsert_system_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.upsert_project_impl(owner, repo, base_path, false, true)
    }

    fn upsert_project_impl(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
        adopted: bool,
        system: bool,
    ) -> Result<i64, rusqlite::Error> {
        let id: i64 = self.conn.query_row(
            "INSERT INTO projects (owner, repo, base_path, adopted, system) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(owner, repo) DO UPDATE SET base_path=excluded.base_path, adopted=excluded.adopted, system=excluded.system
             RETURNING id",
            rusqlite::params![owner, repo, base_path, adopted as i64, system as i64],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_project(id)? {
            self.bus.project_updated(&row);
        }
        Ok(id)
    }

    /// Record project `id`'s GitHub origin ([`ProjectRow::github_origin`]):
    /// the scan's and `service::add_project`'s read of its `origin`, already
    /// filtered by `repo_url::github_origin`. `project:updated` fires only
    /// when the value changed, so a rescan that reads the same origin is
    /// silent.
    pub fn set_project_github_origin(
        &self,
        id: i64,
        origin: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE projects SET github_origin = ?2 WHERE id = ?1 AND github_origin IS NOT ?2",
            rusqlite::params![id, origin],
        )?;
        if n > 0 {
            if let Some(row) = self.get_project(id)? {
                self.bus.project_updated(&row);
            }
        }
        Ok(())
    }

    /// Fill project `id`'s GitHub origin only while it has none recorded:
    /// the backfill for a row that predates migration 207 or that the local
    /// scan never sees (a checkout on a remote host only), from a read of a
    /// checkout of it elsewhere (`service::worktrees`'s remote scan, a clone
    /// onto another host). The local scan stays the one writer that changes
    /// or clears a recorded origin. `project:updated` fires only when it
    /// wrote.
    pub fn fill_project_github_origin(&self, id: i64, origin: &str) -> Result<(), rusqlite::Error> {
        let n = self.conn.execute(
            "UPDATE projects SET github_origin = ?2 WHERE id = ?1 AND github_origin IS NULL",
            rusqlite::params![id, origin],
        )?;
        if n > 0 {
            if let Some(row) = self.get_project(id)? {
                self.bus.project_updated(&row);
            }
        }
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY owner, repo"
        ))?;
        let rows = stmt.query_map([], map_project_row)?;
        rows.collect()
    }

    /// Ids of the projects holding at least one live session — one row per
    /// project whatever its session count, which is what makes this cheaper
    /// than counting in Rust over a joined listing.
    ///
    /// "Live" is `lost_at IS NULL`: the same rows `list_sessions` serves
    /// with its default `include_lost: false`, so a project kept here is
    /// exactly a project some returned session can name.
    pub fn project_ids_with_sessions(&self) -> Result<HashSet<i64>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT project_id FROM sessions
             WHERE project_id IS NOT NULL AND lost_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<HashSet<i64>>>()?)
    }

    /// Single-query variant that builds `Vec<ProjectTreeRow>` in one trip —
    /// eliminates the N+1 of calling `list_worktrees_for_project` per project.
    ///
    /// Projects are ordered: most-recently-used first, NULLs last, then by id.
    /// Within each project worktrees are ordered by id. Only LOCAL worktree
    /// rows are joined: the tree is the local scan, and `refresh_projects`
    /// prunes from this snapshot, so it must never see (or prune) a remote
    /// host's rows.
    pub fn list_projects_joined(
        &self,
    ) -> Result<Vec<crate::service::projects::ProjectTreeRow>, crate::ipc_error::IpcError> {
        // The project columns come first (offset 0, so `map_project_row`
        // reads them as-is); the LEFT JOINed worktree columns follow.
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {cols}, w.id, w.name, w.path, w.branch
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id AND w.host_alias = 'local'
             ORDER BY
               CASE WHEN p.last_session_at IS NULL THEN 1 ELSE 0 END,
               p.last_session_at DESC,
               p.id,
               w.id",
            cols = qualified(PROJECT_COLUMNS, "p")
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                map_project_row(row)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })?;
        let mut out: Vec<crate::service::projects::ProjectTreeRow> = Vec::new();
        let mut last_pid: Option<i64> = None;
        for r in rows {
            let (project, wid, wname, wpath, wbranch) = r?;
            let pid = project.id;
            if last_pid != Some(pid) {
                out.push(crate::service::projects::ProjectTreeRow {
                    project,
                    worktrees: Vec::new(),
                });
                last_pid = Some(pid);
            }
            if let (Some(wid), Some(wname), Some(wpath)) = (wid, wname, wpath) {
                out.last_mut().unwrap().worktrees.push(WorktreeRow {
                    id: wid,
                    project_id: pid,
                    host_alias: crate::service::projects::LOCAL_HOST.to_string(),
                    name: wname,
                    path: wpath,
                    branch: wbranch,
                });
            }
        }
        Ok(out)
    }

    /// Upsert a LOCAL worktree row: the project scan, local hooks, repair,
    /// new sessions. See [`Self::upsert_worktree_on`].
    pub fn upsert_worktree(
        &self,
        project_id: i64,
        name: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        self.upsert_worktree_on(
            crate::service::projects::LOCAL_HOST,
            project_id,
            name,
            path,
            branch,
        )
    }

    /// Upsert a worktree row of `host_alias`, keyed (project, host, name), so
    /// a remote host's worktree never overwrites the local checkout's
    /// same-named row. Every write stamps `updated_at_ms` (migration 026),
    /// which the remote prune's race guard compares with its probe's start.
    /// `worktree:updated` fires for local rows only: the project tree lists
    /// local rows (`list_projects_joined`) and the frontend patches it in
    /// place from these events, so a remote row event would add a row the
    /// next list does not have.
    pub fn upsert_worktree_on(
        &self,
        host_alias: &str,
        project_id: i64,
        name: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let id: i64 = self.conn.query_row(
            "INSERT INTO worktrees (project_id, host_alias, name, path, branch, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(project_id, host_alias, name)
             DO UPDATE SET path=excluded.path, branch=excluded.branch,
                           updated_at_ms=excluded.updated_at_ms
             RETURNING id",
            rusqlite::params![project_id, host_alias, name, path, branch, now_ms],
            |row| row.get(0),
        )?;
        if host_alias == crate::service::projects::LOCAL_HOST {
            if let Some(row) = self.get_worktree_row(id)? {
                self.bus.worktree_updated(&row);
            }
        }
        Ok(id)
    }

    /// When a worktree row was last written, in unix milliseconds (migration
    /// 026). `None` for a row last written before that migration, or for a
    /// row that does not exist.
    pub fn worktree_updated_at_ms(&self, id: i64) -> Result<Option<i64>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT updated_at_ms FROM worktrees WHERE id=?1",
                rusqlite::params![id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// Test hook: backdate or postdate a row's `updated_at_ms`.
    #[cfg(test)]
    pub fn set_worktree_updated_at_ms(&self, id: i64, ms: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE worktrees SET updated_at_ms=?1 WHERE id=?2",
            rusqlite::params![ms, id],
        )?;
        Ok(())
    }

    /// This project's LOCAL worktree rows. Every caller (the refresh prune,
    /// repair, new sessions, the `list_worktrees` tool) works on the local
    /// checkout; a remote host's rows come from [`Self::list_worktrees_on_host`].
    pub fn list_worktrees_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {WORKTREE_COLUMNS} FROM worktrees
             WHERE project_id=?1 AND host_alias='local' ORDER BY name"
        ))?;
        let rows = stmt.query_map(rusqlite::params![project_id], worktree_from_row)?;
        rows.collect()
    }

    /// `list_worktrees`'s answer built in ONE query: every LOCAL worktree row
    /// (optionally restricted to one project, same as
    /// [`Self::list_worktrees_for_project`]) LEFT JOINed with its alive
    /// (`status='running' AND lost_at IS NULL` — [`Self::alive_sessions_for_worktree`]'s
    /// own predicate) session occupants, instead of one
    /// `list_worktrees_for_project` call per project plus one
    /// `alive_sessions_for_worktree` call per worktree. The join condition
    /// matches migration 059's partial index (`idx_sessions_worktree_live`).
    ///
    /// Ordered exactly as the old N+1 path built it — by project (owner,
    /// repo, `list_projects`'s own order), then worktree name
    /// (`list_worktrees_for_project`'s own order), then occupant (host_alias,
    /// tmux_name — `alive_sessions_for_worktree`'s own order) — so
    /// `list_worktrees`'s output is unchanged.
    pub fn worktree_occupancy(
        &self,
        project_id: Option<i64>,
    ) -> Result<Vec<crate::service::worktrees::WorktreeOccupancy>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {wcols}, s.host_alias, s.tmux_name, s.id
               FROM worktrees w
               JOIN projects p ON p.id = w.project_id
          LEFT JOIN sessions s ON s.worktree_id = w.id
                               AND s.status = 'running' AND s.lost_at IS NULL
              WHERE w.host_alias = 'local'
                AND (?1 IS NULL OR w.project_id = ?1)
              ORDER BY p.owner, p.repo, w.name, s.host_alias, s.tmux_name",
            wcols = qualified(WORKTREE_COLUMNS, "w")
        ))?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok((
                worktree_from_row(row)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })?;
        let mut out: Vec<crate::service::worktrees::WorktreeOccupancy> = Vec::new();
        let mut last_wid: Option<i64> = None;
        for r in rows {
            let (w, host, tmux, session_id) = r?;
            if last_wid != Some(w.id) {
                last_wid = Some(w.id);
                out.push(crate::service::worktrees::WorktreeOccupancy {
                    worktree: w,
                    occupants: Vec::new(),
                });
            }
            if let (Some(host_alias), Some(tmux_name), Some(session_id)) = (host, tmux, session_id)
            {
                out.last_mut().unwrap().occupants.push(
                    crate::service::worktrees::WorktreeOccupant {
                        host_alias,
                        tmux_name,
                        session_id,
                    },
                );
            }
        }
        Ok(out)
    }

    /// Every worktree row of one host, across projects: cwd → project
    /// linking (`service::sessions::HostPaths`).
    pub fn list_worktrees_on_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {WORKTREE_COLUMNS} FROM worktrees WHERE host_alias=?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![host_alias], worktree_from_row)?;
        rows.collect()
    }

    /// Delete `host_alias`'s worktree rows at `path` (an `ExitWorktree`
    /// removal on that host), through [`Self::delete_worktree`]: sessions
    /// still pointing at them are cleared and get a `session:updated`, so the
    /// sidebar drops the worktree at once. Returns how many went.
    pub fn delete_worktrees_at(
        &self,
        host_alias: &str,
        path: &str,
    ) -> Result<usize, rusqlite::Error> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id FROM worktrees WHERE host_alias=?1 AND path=?2")?;
            let rows = stmt.query_map(rusqlite::params![host_alias, path], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for id in &ids {
            // Called under the store lock with the host's own spelling of the
            // path: no local resolution here, the stored path only.
            self.delete_worktree(*id, &[])?;
        }
        Ok(ids.len())
    }

    /// Hard-delete one worktree row by id. Emits `worktree:removed`. Returns
    /// the row that was removed (or `None` if it didn't exist).
    ///
    /// Does NOT check for live occupants; the caller is expected to have
    /// done so first (see `service::worktrees::delete_worktree`). Session
    /// rows that still point here have their `worktree_id` cleared so the FK
    /// stays consistent, and get a `session:updated` after the commit so the
    /// sidebar does not keep showing the gone worktree. `fp_keys`: the row's
    /// parent-fingerprint keys, precomputed by the caller off-lock
    /// ([`Self::fingerprint_keys_of_worktree`]).
    pub fn delete_worktree(
        &self,
        id: i64,
        fp_keys: &[String],
    ) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let Some(row) = self.get_worktree_row(id)? else {
            return Ok(None);
        };
        let tx = self.conn.unchecked_transaction()?;
        let touched = session_ids_on_worktree(&tx, id)?;
        tx.execute(
            "UPDATE sessions SET worktree_id=NULL WHERE worktree_id=?1",
            rusqlite::params![id],
        )?;
        // Its recorded parent fingerprint (repair) goes with the row.
        Self::delete_fingerprints(&tx, &row.host_alias, &row.path, fp_keys)?;
        tx.execute("DELETE FROM worktrees WHERE id=?1", rusqlite::params![id])?;
        tx.commit()?;
        self.bus.worktree_removed(id);
        self.emit_sessions_updated(&touched);
        Ok(Some(row))
    }

    /// The keys a worktree row's parent fingerprint may be stored under. A
    /// LOCAL row: the path as given and its canonical form (the nearest
    /// existing ancestor resolved, the missing remainder appended), as the
    /// repair probe records it. A remote row's path is on another machine, so
    /// it is never resolved here: the stored path only.
    ///
    /// Resolving a local path touches the filesystem, which can hang on a
    /// dead NFS mount. So callers run this BEFORE taking the store lock and
    /// pass the result to the delete functions; the store never calls it.
    pub fn fingerprint_keys(host_alias: &str, path: &str) -> Vec<String> {
        let trimmed = path.trim_end_matches('/');
        let mut keys = vec![trimmed.to_string()];
        if host_alias != crate::service::projects::LOCAL_HOST {
            return keys;
        }
        let mut cur = std::path::Path::new(trimmed);
        let mut rest: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if let Ok(mut full) = std::fs::canonicalize(cur) {
                for part in rest.iter().rev() {
                    full.push(part);
                }
                let s = full.to_string_lossy().into_owned();
                if !keys.contains(&s) {
                    keys.push(s);
                }
                break;
            }
            match (cur.file_name(), cur.parent()) {
                (Some(name), Some(parent)) => {
                    rest.push(name.to_os_string());
                    cur = parent;
                }
                _ => break,
            }
        }
        keys
    }

    /// [`Self::fingerprint_keys`] for many rows, by row id. Touches the
    /// filesystem for local rows: call it without the store lock.
    pub fn fingerprint_keys_for<'a>(
        rows: impl IntoIterator<Item = &'a WorktreeRow>,
    ) -> FingerprintKeys {
        rows.into_iter()
            .map(|w| (w.id, Self::fingerprint_keys(&w.host_alias, &w.path)))
            .collect()
    }

    /// One worktree row's fingerprint keys for a caller holding only the
    /// mutex: the row is read under a brief lock, which is released BEFORE
    /// the path is resolved. Empty when the row is gone.
    pub fn fingerprint_keys_of_worktree(store: &std::sync::Mutex<Store>, id: i64) -> Vec<String> {
        let row = match store.lock() {
            Ok(s) => s.get_worktree_row(id).ok().flatten(),
            Err(_) => None,
        };
        row.map(|w| Self::fingerprint_keys(&w.host_alias, &w.path))
            .unwrap_or_default()
    }

    /// A project's worktree rows' fingerprint keys, read and resolved the same
    /// way (lock released before resolving). Covers the LOCAL rows
    /// (`list_worktrees_for_project`): only they have a canonical form to
    /// resolve. A remote row's only key is its stored path, which
    /// `delete_fingerprints` always tries.
    pub fn fingerprint_keys_of_project(
        store: &std::sync::Mutex<Store>,
        project_id: i64,
    ) -> FingerprintKeys {
        let rows = match store.lock() {
            Ok(s) => s.list_worktrees_for_project(project_id).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Self::fingerprint_keys_for(&rows)
    }

    /// Delete the recorded parent fingerprints (repair) of one worktree row,
    /// under the ROW's host: rows are host-scoped (migration 024), and a
    /// fingerprint is keyed by (host, path). Tries the stored path plus the
    /// `keys` the caller precomputed off-lock (a local row's canonical form,
    /// [`Self::fingerprint_keys`]). Never touches the filesystem, so it is
    /// safe under the store lock; a local fingerprint with the same path as a
    /// remote row is left alone.
    fn delete_fingerprints(
        conn: &Connection,
        host_alias: &str,
        path: &str,
        keys: &[String],
    ) -> Result<()> {
        let stored = path.trim_end_matches('/');
        let mut all: Vec<&str> = vec![stored];
        for k in keys {
            if !all.contains(&k.as_str()) {
                all.push(k);
            }
        }
        for key in all {
            conn.execute(
                "DELETE FROM worktree_parent_fingerprints WHERE host_alias=?1 AND wt_path=?2",
                rusqlite::params![host_alias, key],
            )?;
        }
        Ok(())
    }

    /// Return the names + hosts of alive (non-ghost, non-dead) sessions
    /// currently attached to a worktree id. Empty when the worktree is free.
    /// The ids of the alive sessions in one worktree — the same rows as
    /// [`Self::alive_sessions_for_worktree`], by id rather than by
    /// `(host, tmux_name)`.
    ///
    /// Multi-user M1 (T7): `delete_worktree` gates every occupant through
    /// `ViewScope`, which judges a ROW, so the ids are what it needs; the pair
    /// form stays for the refusal message and for the host the removal runs
    /// on.
    pub fn alive_session_ids_for_worktree(
        &self,
        worktree_id: i64,
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id
               FROM sessions
              WHERE worktree_id=?1 AND status='running' AND lost_at IS NULL
              ORDER BY id",
        )?;
        let rows = stmt.query_map(rusqlite::params![worktree_id], |row| row.get::<_, i64>(0))?;
        rows.collect()
    }

    /// Every session row that still OCCUPIES a worktree as the GC and the
    /// recovery paths understand it: the running ones
    /// ([`Self::alive_session_ids_for_worktree`]) plus every row the host
    /// LOST while it was pointing there.
    ///
    /// Multi-user M1 (T8d). `delete_worktree`'s person gate runs over this
    /// set, not over the alive one, and the difference is a whole class of
    /// defect: a host reboot sets `status='ghost', lost_at=now` while the
    /// checkout and its uncommitted work stay on disk, and host-reboot
    /// survival plus `restore_host_sessions` make that a routine state, not
    /// an exotic one. Gated on the alive set alone, the loop iterated nothing
    /// for exactly those rows, so the `Reach::Own` check silently did not run
    /// and anybody could remove another person's checkout — and
    /// `restore_host_sessions` would then have nothing to restore into.
    ///
    /// A row with no `lost_at` and a status other than `running` is NOT an
    /// occupant: it was reaped or dismissed deliberately, and nothing is
    /// waiting to be restored into its tree.
    pub fn occupant_session_ids_for_worktree(
        &self,
        worktree_id: i64,
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id
               FROM sessions
              WHERE worktree_id=?1 AND (status='running' OR lost_at IS NOT NULL)
              ORDER BY id",
        )?;
        let rows = stmt.query_map(rusqlite::params![worktree_id], |row| row.get::<_, i64>(0))?;
        rows.collect()
    }

    pub fn alive_sessions_for_worktree(
        &self,
        worktree_id: i64,
    ) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, tmux_name
               FROM sessions
              WHERE worktree_id=?1 AND status='running' AND lost_at IS NULL
              ORDER BY host_alias, tmux_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![worktree_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    /// Delete this project's worktree rows whose name is not in `keep_names`
    /// (the fresh `git worktree list`). A doomed row may still be referenced
    /// by a session: `sessions.worktree_id` has no ON DELETE and foreign keys
    /// are ON, so a plain DELETE failed and aborted this and every later
    /// refresh (main-first naming replaces the old basename-named main row
    /// while a session still points at it). So, in one transaction, each
    /// doomed row's references move to the surviving row of the same project
    /// with the same canonical path (`canon` maps a stored path to its
    /// canonical form); any left over are cleared, as `delete_worktree` does.
    /// `fp_keys`: parent-fingerprint keys by row id, precomputed off-lock (a
    /// row missing from it drops its stored path only). Emits
    /// `worktree:removed` per deleted row. Returns how many went.
    pub fn delete_worktrees_not_in(
        &self,
        project_id: i64,
        keep_names: &[String],
        canon: impl Fn(&str) -> String,
        fp_keys: &FingerprintKeys,
    ) -> Result<usize, rusqlite::Error> {
        let (keep, doomed): (Vec<WorktreeRow>, Vec<WorktreeRow>) = self
            .list_worktrees_for_project(project_id)?
            .into_iter()
            .partition(|w| keep_names.contains(&w.name));
        if doomed.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut touched: Vec<i64> = Vec::new();
        for d in &doomed {
            touched.extend(session_ids_on_worktree(&tx, d.id)?);
            let key = canon(&d.path);
            if let Some(survivor) = keep.iter().find(|k| canon(&k.path) == key) {
                tx.execute(
                    "UPDATE sessions SET worktree_id=?1 WHERE worktree_id=?2",
                    rusqlite::params![survivor.id, d.id],
                )?;
            }
            tx.execute(
                "UPDATE sessions SET worktree_id=NULL WHERE worktree_id=?1",
                rusqlite::params![d.id],
            )?;
            // Its recorded parent fingerprint (repair) goes with the row, under
            // the row's host, plus the refresh's canonical spelling of it.
            Self::delete_fingerprints(
                &tx,
                &d.host_alias,
                &d.path,
                fp_keys.get(&d.id).map(Vec::as_slice).unwrap_or(&[]),
            )?;
            tx.execute(
                "DELETE FROM worktree_parent_fingerprints WHERE host_alias=?1 AND wt_path=?2",
                rusqlite::params![d.host_alias, key],
            )?;
            tx.execute("DELETE FROM worktrees WHERE id=?1", rusqlite::params![d.id])?;
        }
        tx.commit()?;
        for d in &doomed {
            self.bus.worktree_removed(d.id);
        }
        self.emit_sessions_updated(&touched);
        Ok(doomed.len())
    }

    /// Emit `session:updated` for each of `ids` after a direct UPDATE of
    /// their rows (a re-pointed or cleared `worktree_id`), so the frontend
    /// patches the sidebar at once instead of on the next reconcile pass.
    pub(super) fn emit_sessions_updated(&self, ids: &[i64]) {
        for id in ids {
            // Best-effort: a failed re-read only skips that row's event.
            let _ = self.emit_session(*id);
        }
    }

    /// Delete a project and all its associated sessions and worktrees atomically.
    /// Called after `claude project purge` removes Claude's state on the remote machine.
    ///
    /// Sessions of OTHER projects (or projectless ones) that still point at
    /// one of this project's worktree rows are not this project's to delete;
    /// the old duplicate scan left such references. Their `worktree_id` is
    /// cleared first, or deleting the rows would fail on the foreign key
    /// after the transcripts were already purged, and they get a
    /// `session:updated` after the commit. `fp_keys`: the worktree rows'
    /// parent-fingerprint keys, precomputed off-lock
    /// ([`Self::fingerprint_keys_of_project`]).
    pub fn delete_project(
        &self,
        project_id: i64,
        fp_keys: &FingerprintKeys,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        // The rows that go, with their facts read first, so every client
        // drops them (`killed_payloads`; `delete_host` does the same) instead
        // of listing them until its next full re-list.
        let gone_killed = {
            let ids: Vec<i64> = {
                let mut stmt = tx.prepare("SELECT id FROM sessions WHERE project_id = ?1")?;
                let ids = stmt
                    .query_map(rusqlite::params![project_id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids
            };
            super::sessions::killed_payloads(&tx, &ids)?
        };
        // What dies with the sessions (as `delete_session` does): their
        // timeline. And the recorded parent fingerprints (repair) of the
        // project's worktree rows, each under the ROW's host (rows are
        // host-scoped, migration 024).
        tx.execute(
            "DELETE FROM session_events
              WHERE session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
            rusqlite::params![project_id],
        )?;
        // Tombstone the participant rather than deleting the messages
        // addressed to it, as `delete_session` does — deleting a project
        // must not silently destroy an undelivered inbox.
        tx.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL
              WHERE session_id IN (SELECT id FROM sessions WHERE project_id = ?2)
                AND retired_at IS NULL",
            rusqlite::params![now_unix(), project_id],
        )?;
        let wt_rows: Vec<(i64, String, String)> = {
            let mut stmt =
                tx.prepare("SELECT id, host_alias, path FROM worktrees WHERE project_id = ?1")?;
            let rows = stmt
                .query_map(rusqlite::params![project_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (id, host, path) in &wt_rows {
            let keys = fp_keys.get(id).map(Vec::as_slice).unwrap_or(&[]);
            Self::delete_fingerprints(&tx, host, path, keys)?;
        }
        const CROSS: &str = "project_id IS NOT ?1
               AND worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1)";
        let cross: Vec<i64> = {
            let mut stmt = tx.prepare(&format!("SELECT id FROM sessions WHERE {CROSS}"))?;
            let rows = stmt.query_map(rusqlite::params![project_id], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        tx.execute(
            &format!("UPDATE sessions SET worktree_id = NULL WHERE {CROSS}"),
            rusqlite::params![project_id],
        )?;
        // w05-2: no FK on pull_requests.session_id, and the ids are reused.
        tx.execute(
            "UPDATE pull_requests SET session_id = NULL \
              WHERE session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
            rusqlite::params![project_id],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE project_id = ?1",
            rusqlite::params![project_id],
        )?;
        tx.execute(
            "DELETE FROM worktrees WHERE project_id = ?1",
            rusqlite::params![project_id],
        )?;
        // A routine starts its sessions in this project: it can no longer
        // fire, so it stops with the reason, rather than failing each time
        // its schedule comes round (review r02). Every one of them, an off
        // one included, and it lets go of the id: the next project added
        // takes this reused rowid, and Resume must not start the routine
        // in that other repository (review R6).
        tx.execute(
            "UPDATE routines SET enabled = 0, paused_reason = ?2, next_run_at = NULL, \
               project_id = ?4, updated_at = ?3 WHERE project_id = ?1",
            rusqlite::params![
                project_id,
                super::routines::PAUSED_PROJECT_REMOVED,
                now_unix(),
                super::routines::ROUTINE_NO_PROJECT
            ],
        )?;
        // Its routines' project notebooks go with it (migration 204): a
        // project id is a reused rowid, and the next project to take it
        // must not read another repository's notes.
        tx.execute(
            "DELETE FROM routine_memory WHERE scope = 'project' AND owner_id = ?1",
            rusqlite::params![project_id],
        )?;
        // Its forbidden accounts (migration 217) go too, for the same
        // reason: the FK cascade does it, and this holds without the pragma.
        // Each account is announced after the commit, so no client keeps
        // the dead project id on its row.
        let unblocked = self.accounts_blocked_for_project(project_id)?;
        tx.execute(
            "DELETE FROM account_project_blocks WHERE project_id = ?1",
            rusqlite::params![project_id],
        )?;
        // Two more references a reused id would inherit (w05-3): its ticket
        // space (the next project would number its tickets on from this
        // one's prefix), detached but kept so old keys still resolve; and
        // its pull requests' project (no FK).
        super::ticket_keys::retire_project_ticket_space_in(&tx, project_id)?;
        tx.execute(
            "UPDATE pull_requests SET project_id = NULL WHERE project_id = ?1",
            rusqlite::params![project_id],
        )?;
        // A work link's snapshot of its project (migration 046, no FK; the
        // retire trigger has just stamped it on this project's links). The
        // id is reused, so it would resume the link's task, and default a
        // new start of that ticket prefix, into whatever project is added
        // next under it — the routines' R6 bug, here.
        tx.execute(
            "UPDATE work_links SET snap_project_id = NULL WHERE snap_project_id = ?1",
            rusqlite::params![project_id],
        )?;
        tx.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        )?;
        tx.commit()?;
        for killed in gone_killed {
            self.bus.session_killed(killed);
        }
        self.emit_sessions_updated(&cross);
        self.announce_accounts(&unblocked);
        Ok(())
    }

    /// Delete a project row unless a session references it, either directly
    /// (`project_id`) or through one of its worktree rows (`worktree_id`).
    /// The worktree check matters: the old duplicate-scan bug left sessions
    /// whose `project_id` is another project (or NULL) pointing at this
    /// project's worktree rows, and `delete_project` removing those rows
    /// would violate the foreign key. Returns whether the row went.
    /// `refresh_projects` uses it for stale rows outside the projects root and
    /// for duplicate rows naming a checkout another project owns.
    pub fn delete_project_if_unused(
        &self,
        project_id: i64,
        fp_keys: &FingerprintKeys,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        let in_use: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions
               WHERE project_id = ?1
                  OR worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1))",
            rusqlite::params![project_id],
            |r| r.get(0),
        )?;
        if in_use {
            return Ok(false);
        }
        self.delete_project(project_id, fp_keys)?;
        Ok(true)
    }

    /// Fold project `from` into project `into` in ONE transaction, then
    /// delete the `from` row. For a repository that changed owner or name
    /// and was registered a second time: two projects over one checkout,
    /// between which a session moved with whichever worktree row was newer
    /// (2026-11-16, `find_project_id_for_path`).
    ///
    /// Rules. A worktree `into` already has under the same host and name
    /// keeps `into`'s row, and the sessions on `from`'s twin point at it;
    /// the other worktrees move. Sessions, pull requests, work items and
    /// links, routines, agent rules, mission lanes and PR-shepherd merges
    /// move. A start rule, PR-shepherd rule, mission repo, forbidden account
    /// or project-scope environment variable `into` already has keeps
    /// `into`'s; the rest move
    /// ([`PROJECT_ID_COLUMNS`]). The choices that name the repository in
    /// words move too, `into`'s own winning: its pin, hide and group
    /// (`project_picks`), the local workspaces synced from it (one `into`
    /// already has for the same host and worktree is kept and `from`'s
    /// dropped), and a fleet agent's input whose value is exactly `from`'s
    /// `owner/repo` (`agent_rules.config`). History that names it
    /// (`org_rules`, the review tables) is left as it is. The project notebooks of `from`'s routines and its ticket space
    /// (`PROJECT_REFS_BY_OTHER_NAME`) become `into`'s unless it has its
    /// own. Emits `session:updated` per moved or re-pointed session,
    /// `worktree:updated` per moved local worktree row, `worktree:removed`
    /// per folded one, `account:upserted` per account whose forbidden
    /// projects changed, and `project:updated` for `into`.
    ///
    /// Refused (`E_INVALID`) when the two rows are not one checkout: their
    /// base paths differ, or a twin worktree has another path; and
    /// (`E_INVALID_STATE`) while `from` has a live session, as
    /// `forget_project` is.
    pub fn merge_project(
        &self,
        from: i64,
        into: i64,
    ) -> Result<ProjectMergeReport, crate::ipc_error::IpcError> {
        self.merge_project_with(from, into, false)
    }

    /// [`Self::merge_project`]; with `dry_run` the transaction runs and its
    /// report is built, then it is rolled back and nothing is emitted: what
    /// the merge would move, with nothing written.
    pub fn merge_project_with(
        &self,
        from: i64,
        into: i64,
        dry_run: bool,
    ) -> Result<ProjectMergeReport, crate::ipc_error::IpcError> {
        self.merge_project_checked(from, into, dry_run, &HashSet::new())
    }

    /// [`Self::merge_project_with`], where `gone` names the LOCAL paths of
    /// `from` (its base path, its `local` worktrees) that are no longer a
    /// checkout of their own: removed from disk, or the same directory as
    /// `into`'s. Such a path does not make the two rows "two checkouts"
    /// (review w01-2: after a github-layout rename the old row keeps its
    /// old base path for good, so the merge was refused even once the old
    /// checkout was gone). The caller stats the disk; the store does not.
    pub fn merge_project_checked(
        &self,
        from: i64,
        into: i64,
        dry_run: bool,
        gone: &HashSet<String>,
    ) -> Result<ProjectMergeReport, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        if from == into {
            return Err(IpcError::new(
                codes::E_INVALID,
                "from and into are the same project",
            ));
        }
        let mut names = Vec::with_capacity(2);
        let mut bases = Vec::with_capacity(2);
        for id in [from, into] {
            let Some(p) = fetch_project(&self.conn, id)? else {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("project {id} not found"),
                ));
            };
            if p.system {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("project {id} is a system project; it has no repository to merge"),
                ));
            }
            names.push(format!("{}/{}", p.owner, p.repo));
            bases.push(p.base_path.trim_end_matches('/').to_string());
        }
        let into_name = names.pop().unwrap_or_default();
        let from_name = names.pop().unwrap_or_default();
        // The merge is for two rows over ONE checkout (docs/hub.md). Two
        // directories (the github layout keeps `owner/repo` in the path)
        // would come apart again on the next scan, which re-registers the
        // old one from its path, and a session on a folded twin would carry
        // the other directory's worktree row (review w05-4).
        if bases[0] != bases[1] && !gone.contains(&bases[0]) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{from_name} ({}) and {into_name} ({}) are two checkouts, not one; remove or rename the old checkout first, then merge (if it is already gone from a host fleet cannot look at from here, forget {from_name} instead)",
                    bases[0], bases[1]
                ),
            ));
        }
        let crossed_twin: Option<(String, String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT w1.host_alias, w1.name, w1.path, w2.path,
                        EXISTS (SELECT 1 FROM sessions s WHERE s.worktree_id = w1.id)
                   FROM worktrees w1
                   JOIN worktrees w2 ON w2.project_id = ?2 AND w2.host_alias = w1.host_alias
                    AND w2.name = w1.name
                  WHERE w1.project_id = ?1 AND w1.path IS NOT w2.path
                  ORDER BY w1.id",
            )?;
            let rows = stmt.query_map(rusqlite::params![from, into], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            let mut first = None;
            for row in rows {
                let (host, name, a, b, used): (String, String, String, String, bool) = row?;
                if host == "local" && gone.contains(&a) {
                    continue;
                }
                // A remote twin no session sits on folds into `into`'s row
                // like any other twin: nothing carries its directory over,
                // and fleet cannot look at that host from here to tell
                // whether the directory is still there (w01-6).
                if host != "local" && !used {
                    continue;
                }
                first = Some((host, name, a, b));
                break;
            }
            first
        };
        if let Some((host, name, a, b)) = crossed_twin {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "worktree {name} on {host} is {a} under {from_name} but {b} under {into_name}: two checkouts, not one; remove or rename the old checkout first, then merge (or Clean up the session on that worktree, or forget {from_name} instead)"
                ),
            ));
        }
        // A live session of `from` is refused, as `forget_project` refuses
        // one: a reconcile pass that read the project list before the merge
        // writes `from`'s id after it, and the foreign key fails that host's
        // whole write (review w05-7).
        let live = self.project_live_session_count(from)?;
        if live > 0 {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "{from_name} has {live} live session(s); kill or move them first, then merge"
                ),
            ));
        }
        // Its forbidden accounts change rows: announced after the commit, so
        // no client keeps `from`'s dead id on them (review w05-3 / w20-2).
        let reblocked = self.accounts_blocked_for_project(from)?;
        let ids = |conn: &Connection, sql: &str, params: &[i64]| -> rusqlite::Result<Vec<i64>> {
            let mut stmt = conn.prepare(sql)?;
            let rows =
                stmt.query_map(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))?;
            rows.collect()
        };
        let tx = self.conn.unchecked_transaction()?;
        // Worktrees: move the ones `into` lacks; point sessions at the twin
        // for the ones it has, then drop the leftovers.
        const NO_TWIN: &str = "NOT EXISTS (
               SELECT 1 FROM worktrees w2 WHERE w2.project_id = ?2
                  AND w2.host_alias = worktrees.host_alias AND w2.name = worktrees.name)";
        let wt_moved = ids(
            &tx,
            &format!("SELECT id FROM worktrees WHERE project_id = ?1 AND {NO_TWIN}"),
            &[from, into],
        )?;
        let wt_folded = ids(
            &tx,
            &format!("SELECT id FROM worktrees WHERE project_id = ?1 AND NOT {NO_TWIN}"),
            &[from, into],
        )?;
        // Every session the merge touches: `from`'s own, and any other on
        // one of `from`'s worktree rows (a link that was already crossed).
        let touched = ids(
            &tx,
            "SELECT id FROM sessions WHERE project_id = ?1
                OR worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1)",
            &[from],
        )?;
        let sessions_moved = ids(
            &tx,
            "SELECT id FROM sessions WHERE project_id = ?1",
            &[from],
        )?
        .len();
        tx.execute(
            &format!("UPDATE worktrees SET project_id = ?2 WHERE project_id = ?1 AND {NO_TWIN}"),
            rusqlite::params![from, into],
        )?;
        // What is left under `from` now are the twins.
        for (table, column) in WORKTREE_ID_COLUMNS {
            tx.execute(
                &format!(
                    "UPDATE {table} SET {column} = (
                       SELECT w2.id FROM worktrees w1 JOIN worktrees w2
                         ON w2.project_id = ?2 AND w2.host_alias = w1.host_alias
                        AND w2.name = w1.name
                       WHERE w1.id = {table}.{column})
                     WHERE {column} IN (SELECT id FROM worktrees WHERE project_id = ?1)"
                ),
                rusqlite::params![from, into],
            )?;
        }
        tx.execute("DELETE FROM worktrees WHERE project_id = ?1", [from])?;
        let mut other_rows_moved = 0;
        for (table, column, how) in PROJECT_ID_COLUMNS {
            let n = match how {
                ProjectRef::Worktrees => continue,
                ProjectRef::Moves => tx.execute(
                    &format!("UPDATE {table} SET {column} = ?2 WHERE {column} = ?1"),
                    rusqlite::params![from, into],
                )?,
                ProjectRef::IntoWins => {
                    let n = tx.execute(
                        &format!("UPDATE OR IGNORE {table} SET {column} = ?2 WHERE {column} = ?1"),
                        rusqlite::params![from, into],
                    )?;
                    tx.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), [from])?;
                    n
                }
            };
            if *table != "sessions" {
                other_rows_moved += n;
            }
        }
        // Project-scope environment (`owner/repo`): into's own entry wins on
        // a name clash.
        tx.execute(
            "UPDATE OR IGNORE env_vars SET scope_key = ?2 WHERE scope = 'project' AND scope_key = ?1",
            rusqlite::params![from_name, into_name],
        )?;
        tx.execute(
            "DELETE FROM env_vars WHERE scope = 'project' AND scope_key = ?1",
            [&from_name],
        )?;
        // Project notebooks of its routines (migration 204), which now read
        // `into`'s: into's own wins per circle, as with the variables above.
        // Left behind, the next project to reuse `from`'s rowid would read
        // them (review w05-1).
        tx.execute(
            "UPDATE OR IGNORE routine_memory SET owner_id = ?2 \
              WHERE scope = 'project' AND owner_id = ?1",
            rusqlite::params![from, into],
        )?;
        tx.execute(
            "DELETE FROM routine_memory WHERE scope = 'project' AND owner_id = ?1",
            [from],
        )?;
        // Its ticket space, whose prefix the moved items' keys carry: it
        // becomes `into`'s unless `into` has one already, so `into` does not
        // take a second prefix beside it (review w05-2). A space left behind
        // still resolves its keys, and the report names it.
        tx.execute(
            "UPDATE OR IGNORE ticket_spaces SET scope = 'project:' || ?2 \
              WHERE scope = 'project:' || ?1",
            rusqlite::params![from, into],
        )?;
        let ticket_space_left: Option<String> = tx
            .query_row(
                "SELECT prefix FROM ticket_spaces WHERE scope = 'project:' || ?1",
                [from],
                |r| r.get(0),
            )
            .optional()?;
        // The person's pin, hide and group (`project_picks`, keyed by the
        // words): into's own wins.
        let (fo, fr) = from_name.split_once('/').unwrap_or((&from_name, ""));
        let (io, ir) = into_name.split_once('/').unwrap_or((&into_name, ""));
        let picks_moved = tx.execute(
            "UPDATE OR IGNORE project_picks SET owner = ?3, repo = ?4 \
              WHERE owner = ?1 AND repo = ?2",
            rusqlite::params![fo, fr, io, ir],
        )?;
        tx.execute(
            "DELETE FROM project_picks WHERE owner = ?1 AND repo = ?2",
            rusqlite::params![fo, fr],
        )?;
        // Local workspaces synced from it: one into already has for the same
        // host and worktree wins, and from's twin goes (its sync state with
        // it; the local files stay where they are).
        let workspaces_moved = tx.execute(
            "UPDATE OR IGNORE local_workspaces SET owner = ?3, repo = ?4 \
              WHERE owner = ?1 AND repo = ?2",
            rusqlite::params![fo, fr, io, ir],
        )?;
        let workspaces_dropped = tx.execute(
            "DELETE FROM local_workspaces WHERE owner = ?1 AND repo = ?2",
            rusqlite::params![fo, fr],
        )?;
        // A fleet agent's project input is its `owner/repo` (validated so
        // by `fleet_agents::grant::check_values`): a top-level config value
        // that is exactly from's name now names into.
        let agent_inputs_moved = {
            let rules: Vec<(i64, String)> = {
                let mut stmt = tx.prepare("SELECT id, config FROM agent_rules")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut n = 0;
            for (id, config) in rules {
                if let Some(next) = rename_config_inputs(&config, &from_name, &into_name) {
                    tx.execute(
                        "UPDATE agent_rules SET config = ?2 WHERE id = ?1",
                        rusqlite::params![id, next],
                    )?;
                    n += 1;
                }
            }
            n
        };
        // Two more settings name the repo in words (review w03-3): a review
        // rule's scope `gh:<owner>/<repo>` and a routine's PR-event filter.
        // A review rule into already has for its new scope wins; from's
        // twin goes (its jobs keep their rows, `rule_id` set NULL).
        let review_rules_moved = tx.execute(
            "UPDATE OR IGNORE agent_rules SET scope = 'gh:' || ?2 \
              WHERE agent = 'review' AND lower(scope) = lower('gh:' || ?1)",
            rusqlite::params![from_name, into_name],
        )?;
        tx.execute(
            "DELETE FROM agent_rules WHERE agent = 'review' AND lower(scope) = lower('gh:' || ?1)",
            [&from_name],
        )?;
        let routine_filters_moved = tx.execute(
            "UPDATE routines SET event_repo = ?2 WHERE lower(event_repo) = lower(?1)",
            rusqlite::params![from_name, into_name],
        )?;
        // A space from kept (into has its own) is detached, as
        // `delete_project` does: a project that reuses from's id must not
        // number its tickets on from it (w05-3).
        super::ticket_keys::retire_project_ticket_space_in(&tx, from)?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [from])?;
        let report = ProjectMergeReport {
            from,
            into,
            from_name,
            into_name,
            sessions_moved,
            worktrees_moved: wt_moved.len(),
            worktrees_folded: wt_folded.len(),
            other_rows_moved,
            picks_moved,
            workspaces_moved,
            workspaces_dropped,
            agent_inputs_moved,
            review_rules_moved,
            routine_filters_moved,
            ticket_space_left,
            dry_run,
        };
        if dry_run {
            // Dropping the transaction rolls it back.
            drop(tx);
            return Ok(report);
        }
        tx.commit()?;
        for id in &wt_folded {
            self.bus.worktree_removed(*id);
        }
        // Local rows only, as `upsert_worktree_on` emits: the project tree
        // lists local rows, and a remote row event would add a row the next
        // list does not have (review w05-3).
        for id in &wt_moved {
            if let Some(row) = self.get_worktree_row(*id)? {
                if row.host_alias == crate::service::projects::LOCAL_HOST {
                    self.bus.worktree_updated(&row);
                }
            }
        }
        self.emit_sessions_updated(&touched);
        self.announce_accounts(&reblocked);
        if let Some(row) = fetch_project(&self.conn, into)? {
            self.bus.project_updated(&row);
        }
        Ok(report)
    }

    /// Delete one worktree row unless any session row (alive, ghost or dead)
    /// references it. Emits `worktree:removed` when the row goes. Returns
    /// whether it went.
    pub fn delete_worktree_if_unused(&self, id: i64) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "DELETE FROM worktrees WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM sessions WHERE worktree_id = ?1)",
            rusqlite::params![id],
        )?;
        if n > 0 {
            self.bus.worktree_removed(id);
        }
        Ok(n > 0)
    }

    /// Two rows naming one checkout on one host (data-sync F7: 12 such
    /// pairs, linked worktrees scanned as repos). Keep the row whose
    /// project's `base_path` is a prefix of the path (the owner), else the
    /// lowest id; re-point sessions at it; delete the rest. Returns rows
    /// deleted. Emits `worktree:removed` per deleted row and
    /// `session:updated` per re-pointed session.
    pub fn dedupe_worktree_paths(&self) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let groups: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT host_alias, path FROM worktrees GROUP BY host_alias, path HAVING COUNT(*) > 1",
            )?;
            let g = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            g
        };
        let mut removed: Vec<i64> = Vec::new();
        let mut touched: Vec<i64> = Vec::new();
        for (host, path) in groups {
            let rows: Vec<(i64, i64, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT w.id, w.project_id, p.base_path FROM worktrees w
                       JOIN projects p ON p.id = w.project_id
                      WHERE w.host_alias = ?1 AND w.path = ?2 ORDER BY w.id",
                )?;
                let v = stmt
                    .query_map(rusqlite::params![host, path], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                v
            };
            let Some(keep) = rows
                .iter()
                .find(|(_, _, base)| path.starts_with(&format!("{}/", base.trim_end_matches('/'))))
                .or(rows.first())
                .map(|(id, pid, _)| (*id, *pid))
            else {
                continue;
            };
            for (id, _, _) in rows.iter().filter(|(id, _, _)| *id != keep.0) {
                let ids: Vec<i64> = {
                    let mut stmt = tx.prepare("SELECT id FROM sessions WHERE worktree_id = ?1")?;
                    let v = stmt
                        .query_map([id], |r| r.get::<_, i64>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    v
                };
                touched.extend(ids);
                tx.execute(
                    "UPDATE sessions SET worktree_id = ?2, project_id = ?3 WHERE worktree_id = ?1",
                    rusqlite::params![id, keep.0, keep.1],
                )?;
                tx.execute("DELETE FROM worktrees WHERE id = ?1", [id])?;
                removed.push(*id);
            }
        }
        tx.commit()?;
        for id in &removed {
            self.bus.worktree_removed(*id);
        }
        self.emit_sessions_updated(&touched);
        Ok(removed.len())
    }

    /// Live (non-ghost) session rows on the project or its worktrees.
    pub fn project_live_session_count(&self, project_id: i64) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE status != 'ghost' AND (project_id = ?1
               OR worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1))",
            [project_id],
            |r| r.get(0),
        )
    }

    /// Delete the project's ghost rows (events with them, inbox tombstoned,
    /// as `delete_session` does) so `delete_project_if_unused` can proceed.
    /// Returns the ids.
    pub fn delete_project_ghost_sessions(
        &self,
        project_id: i64,
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM sessions WHERE status = 'ghost' AND (project_id = ?1
                   OR worktree_id IN (SELECT id FROM worktrees WHERE project_id = ?1))",
            )?;
            let v = stmt
                .query_map([project_id], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            v
        };
        for id in &ids {
            self.delete_session(*id)?;
        }
        Ok(ids)
    }

    /// After a scan of `host_alias`'s checkout of `project_id`: drop that
    /// host's rows the scan did not report, except rows a session still
    /// points at (any status — the FK must stay valid). Local rows are never
    /// touched. A successful scan always reports the root worktree, so an
    /// empty `keep_names` means the scan failed and must not wipe the cache.
    /// Emits `worktree:removed` per dropped row like
    /// [`Self::delete_worktree_if_unused`]. Returns how many went.
    pub fn delete_host_worktrees_not_in(
        &self,
        host_alias: &str,
        project_id: i64,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if host_alias == crate::service::projects::LOCAL_HOST {
            return Ok(0);
        }
        if keep_names.is_empty() {
            return Ok(0);
        }
        let doomed: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id, name, path FROM worktrees WHERE host_alias=?1 AND project_id=?2",
            )?;
            let rows: Result<Vec<(i64, String, String)>, rusqlite::Error> = stmt
                .query_map(rusqlite::params![host_alias, project_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect();
            rows?
                .into_iter()
                .filter(|(_, name, _)| !keep_names.contains(name))
                .map(|(id, _, path)| (id, path))
                .collect()
        };
        let mut n = 0;
        // Each delete is independent and self-conditional (only a row no
        // session references goes), and delete_worktree_if_unused emits
        // worktree:removed inside itself — an outer transaction's rollback
        // would emit events for rows that then come back. A partial prune is
        // idempotent: the next scan retries whatever this one left behind.
        for (id, path) in doomed {
            if self.delete_worktree_if_unused(id)? {
                Self::delete_fingerprints(&self.conn, host_alias, &path, &[])?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Remember the parent directory's `dev:inode` of a worktree a probe found
    /// healthy (migration 023). Keyed by host and canonical worktree path.
    pub fn record_parent_fingerprint(
        &self,
        host_alias: &str,
        wt_path: &str,
        parent_fp: &str,
        recorded_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktree_parent_fingerprints (host_alias, wt_path, parent_fp, recorded_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(host_alias, wt_path)
             DO UPDATE SET parent_fp = excluded.parent_fp, recorded_at = excluded.recorded_at",
            rusqlite::params![host_alias, wt_path, parent_fp, recorded_at],
        )?;
        Ok(())
    }

    /// The recorded parent `dev:inode` of a worktree, if any.
    pub fn parent_fingerprint(
        &self,
        host_alias: &str,
        wt_path: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT parent_fp FROM worktree_parent_fingerprints
                 WHERE host_alias = ?1 AND wt_path = ?2",
                rusqlite::params![host_alias, wt_path],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn worktree_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT path FROM worktrees WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    /// The id of `host_alias`'s worktree row `name` in `project_id`, if one
    /// is recorded: what a `new_worktree` start under that name lands in.
    pub fn worktree_id_named(
        &self,
        host_alias: &str,
        project_id: i64,
        name: &str,
    ) -> Result<Option<i64>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id FROM worktrees WHERE project_id = ?1 AND host_alias = ?2 AND name = ?3",
                rusqlite::params![project_id, host_alias, name],
                |row| row.get(0),
            )
            .optional()
    }

    /// Worktree's logical name (the leaf used in remote
    /// `~/projects/.../.claude/worktrees/<name>` paths). The on-host `path`
    /// column is local-machine-only and unusable for remote rebuilds.
    pub fn worktree_name(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT name FROM worktrees WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn project_base_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT base_path FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }
}

/// PURE: `config` (a fleet agent's input values, a JSON object) with every
/// top-level string value that is exactly `from` (an `owner/repo`) set to
/// `into`; `None` when no value changes or it is not an object.
fn rename_config_inputs(config: &str, from: &str, into: &str) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(config).ok()?;
    let mut changed = false;
    for val in v.as_object_mut()?.values_mut() {
        if val.as_str().is_some_and(|s| s.trim() == from) {
            *val = serde_json::Value::String(into.to_string());
            changed = true;
        }
    }
    changed.then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge's column list against the schema itself: every column that
    /// is named as a project id or is a foreign key to `projects` must be
    /// in [`PROJECT_ID_COLUMNS`], and nothing listed may be gone. A table
    /// added later that names a project fails here until the merge is told
    /// what to do with it.
    #[test]
    fn a_project_merge_covers_every_project_column() {
        use std::collections::BTreeSet;
        let s = Store::open_in_memory().unwrap();
        let conn = s.conn_for_test();
        let col = |sql: &str, idx: usize| -> Vec<String> {
            let mut st = conn.prepare(sql).unwrap();
            let rows = st.query_map([], |r| r.get::<_, String>(idx)).unwrap();
            rows.collect::<Result<_, _>>().unwrap()
        };
        let tables = col(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            0,
        );
        assert!(tables.len() > 50, "the migrated schema, not an empty one");
        let mut found: BTreeSet<(String, String)> = BTreeSet::new();
        for t in &tables {
            for c in col(&format!("PRAGMA table_info(\"{t}\")"), 1) {
                // `orchestration_project_id` names a mission, not a project.
                let names_a_project = c == "project_id"
                    || (c.ends_with("_project_id") && c != "orchestration_project_id");
                if names_a_project {
                    found.insert((t.clone(), c));
                }
            }
            let mut st = conn
                .prepare(&format!("PRAGMA foreign_key_list(\"{t}\")"))
                .unwrap();
            let fks: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for (target, from_col) in fks {
                if target == "projects" {
                    found.insert((t.clone(), from_col));
                }
            }
        }
        let listed: BTreeSet<(String, String)> = PROJECT_ID_COLUMNS
            .iter()
            .map(|(t, c, _)| (t.to_string(), c.to_string()))
            .collect();
        assert_eq!(
            found, listed,
            "PROJECT_ID_COLUMNS must name every project-id column, and only those"
        );
        // The references by another name the scan above cannot see: each
        // must still exist, or the merge's hand-written re-homing of it is
        // stale (review w05-1 / w20-1).
        for (t, c) in PROJECT_REFS_BY_OTHER_NAME {
            assert!(
                col(&format!("PRAGMA table_info(\"{t}\")"), 1).contains(&c.to_string()),
                "{t}.{c} is listed in PROJECT_REFS_BY_OTHER_NAME but not in the schema"
            );
        }
        // A notebook is keyed by `(scope, owner_id)`: a new scope that names
        // a project must be re-homed by the merge too.
        let scope_check: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'routine_memory'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            scope_check.contains("scope IN ('routine', 'project')"),
            "routine_memory's scopes changed; tell merge_project: {scope_check}"
        );
        // The same for what points at a worktree row, which a folded twin
        // must hand over.
        let mut found: BTreeSet<(String, String)> = BTreeSet::new();
        for t in &tables {
            for c in col(&format!("PRAGMA table_info(\"{t}\")"), 1) {
                if c == "worktree_id" || c.ends_with("_worktree_id") {
                    found.insert((t.clone(), c));
                }
            }
            let mut st = conn
                .prepare(&format!("PRAGMA foreign_key_list(\"{t}\")"))
                .unwrap();
            let fks: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for (target, from_col) in fks {
                if target == "worktrees" {
                    found.insert((t.clone(), from_col));
                }
            }
        }
        let listed: BTreeSet<(String, String)> = WORKTREE_ID_COLUMNS
            .iter()
            .map(|(t, c)| (t.to_string(), c.to_string()))
            .collect();
        assert_eq!(
            found, listed,
            "WORKTREE_ID_COLUMNS must name every worktree-id column, and only those"
        );
    }

    /// A repository that changed owner, registered twice over one checkout:
    /// the merge leaves one project holding every session and worktree, a
    /// twin worktree folded into the survivor's row, and nothing pointing at
    /// the project that went. The crossed link of 2026-11-16 (a session of
    /// the new project on the old project's worktree row) ends on the
    /// survivor's row too.
    #[test]
    fn a_merge_folds_one_project_into_the_other() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("mac").unwrap();
        s.upsert_host(crate::service::projects::LOCAL_HOST).unwrap();
        // One checkout, registered under both names (the flat layout).
        let old = s.upsert_project("frantisek", "app", "/p/app").unwrap();
        let new = s.upsert_project("sales-twins", "app", "/p/app/").unwrap();
        let wt = |pid, name: &str| {
            s.upsert_worktree_on("mac", pid, name, &format!("/co/.worktrees/{name}"), None)
                .unwrap()
        };
        let (old_feat, old_only, new_feat) =
            (wt(old, "feat"), wt(old, "only-old"), wt(new, "feat"));
        let local_only = s
            .upsert_worktree_on(
                crate::service::projects::LOCAL_HOST,
                old,
                "local-only",
                "/p/app/.worktrees/local-only",
                None,
            )
            .unwrap();
        // The old project's sessions are gone (ghost rows): a live one is
        // refused, `a_merge_refuses_a_live_session_of_the_old_project`.
        let sess = |name: &str, pid, wid, status| {
            s.upsert_session(name, "mac", Some(pid), Some(wid), 1, 1, status, None)
                .unwrap()
        };
        let on_twin = sess("on-twin", old, old_feat, "ghost");
        let on_own = sess("on-own", old, old_only, "ghost");
        let already_new = sess("already-new", new, new_feat, "running");
        let crossed = sess("crossed", new, old_feat, "ghost");
        // Project notebooks of its routines: the survivor's wins in a circle
        // both have, the old one's moves in a circle only it has.
        for (pid, circle, body) in [(old, 0, "old"), (new, 0, "new"), (old, 5, "old-5")] {
            s.set_routine_memory("project", (pid, circle), body, "person", None, 1)
                .unwrap();
        }
        // The old project's ticket space; the survivor has none yet.
        let space = s.project_ticket_space(old).unwrap();
        account_row(&s, "acct-a");
        account_row(&s, "acct-b");
        // A start rule both have (the survivor's is kept) and one only the
        // old project has (it moves); the same for a project-scope variable.
        for (pid, pattern) in [(old, "APP-"), (new, "APP-"), (old, "OLD-")] {
            s.insert_start_rule(None, None, pattern, pid, None, "active", 1, 1)
                .unwrap();
        }
        let plain = |v: &str| crate::store::EnvValue::Plain(v.to_string());
        s.set_env_var("project", "frantisek/app", "SHARED", &plain("old"))
            .unwrap();
        s.set_env_var("project", "sales-twins/app", "SHARED", &plain("new"))
            .unwrap();
        s.set_env_var("project", "frantisek/app", "ONLY_OLD", &plain("x"))
            .unwrap();

        // Forbidden accounts add up: one both forbid, one only the old did.
        for (account, pid) in [("acct-a", old), ("acct-a", new), ("acct-b", old)] {
            s.conn_for_test()
                .execute(
                    "INSERT INTO account_project_blocks (account_uuid, project_id, created_at) \
                     VALUES (?1, ?2, 1)",
                    rusqlite::params![account, pid],
                )
                .unwrap();
        }

        bus.take();
        let rep = s.merge_project(old, new).unwrap();
        assert_eq!(
            rep,
            ProjectMergeReport {
                from: old,
                into: new,
                from_name: "frantisek/app".into(),
                into_name: "sales-twins/app".into(),
                sessions_moved: 2,
                worktrees_moved: 2,
                worktrees_folded: 1,
                // The start rule and the forbidden account only the old had.
                other_rows_moved: 2,
                picks_moved: 0,
                workspaces_moved: 0,
                workspaces_dropped: 0,
                agent_inputs_moved: 0,
                review_rules_moved: 0,
                routine_filters_moved: 0,
                ticket_space_left: None,
                dry_run: false,
            }
        );
        let events = bus.take();
        for uuid in ["acct-a", "acct-b"] {
            assert!(
                events.contains(&format!("account:upserted:{uuid}")),
                "{uuid} is announced: {events:?}"
            );
        }
        assert!(
            events.contains(&format!("worktree:updated:{local_only}")),
            "{events:?}"
        );
        assert!(
            !events.contains(&format!("worktree:updated:{old_only}")),
            "no event for a moved remote row: {events:?}"
        );
        assert_eq!(
            s.get_account_by_uuid("acct-b")
                .unwrap()
                .unwrap()
                .forbidden_project_ids,
            vec![new]
        );
        let notebook = |pid, circle| {
            s.get_routine_memory("project", pid, circle)
                .unwrap()
                .map(|m| m.body)
        };
        assert_eq!(
            notebook(new, 0).as_deref(),
            Some("new"),
            "the survivor's wins"
        );
        assert_eq!(notebook(new, 5).as_deref(), Some("old-5"), "the rest moves");
        assert_eq!((notebook(old, 0), notebook(old, 5)), (None, None));
        assert_eq!(
            s.project_ticket_space(new).unwrap().id,
            space.id,
            "the survivor takes the old space and its prefix"
        );
        assert!(
            s.get_project(old).unwrap().is_none(),
            "the old project is gone"
        );
        let row = |id| {
            let r: (Option<i64>, Option<i64>) = s
                .conn_for_test()
                .query_row(
                    "SELECT project_id, worktree_id FROM sessions WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            r
        };
        assert_eq!(row(on_twin), (Some(new), Some(new_feat)));
        assert_eq!(row(on_own), (Some(new), Some(old_only)));
        assert_eq!(row(already_new), (Some(new), Some(new_feat)));
        assert_eq!(row(crossed), (Some(new), Some(new_feat)));
        assert!(
            s.get_worktree_row(old_feat).unwrap().is_none(),
            "the twin is folded"
        );
        assert_eq!(
            s.get_worktree_row(old_only).unwrap().unwrap().project_id,
            new
        );
        // Nothing names the old project any more, in any listed table.
        for (table, column, _) in PROJECT_ID_COLUMNS {
            let n: i64 = s
                .conn_for_test()
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                    [old],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "{table}.{column} still names the merged project");
        }
        let rules: Vec<String> = {
            let mut st = s
                .conn_for_test()
                .prepare("SELECT pattern FROM start_rules WHERE project_id = ?1 ORDER BY pattern")
                .unwrap();
            let v = st.query_map([new], |r| r.get(0)).unwrap();
            v.collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(
            rules,
            ["APP-", "OLD-"],
            "the survivor's rule, plus the one it lacked"
        );
        let blocked: Vec<String> = {
            let mut st = s
                .conn_for_test()
                .prepare(
                    "SELECT account_uuid FROM account_project_blocks WHERE project_id = ?1 \
                     ORDER BY account_uuid",
                )
                .unwrap();
            let v = st.query_map([new], |r| r.get(0)).unwrap();
            v.collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(
            blocked,
            ["acct-a", "acct-b"],
            "every account either forbade"
        );
        let vars: Vec<(String, String)> = {
            let mut st = s
                .conn_for_test()
                .prepare(
                    "SELECT name, value FROM env_vars WHERE scope = 'project' \
                     ORDER BY scope_key, name",
                )
                .unwrap();
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            v.collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(
            vars,
            [
                ("ONLY_OLD".into(), "x".into()),
                ("SHARED".into(), "new".into())
            ],
            "all under the survivor's name; its own value wins"
        );
    }

    /// The choices keyed by `owner/repo` follow the merge, the survivor's
    /// own winning (review w05-8): picks, local workspaces and a fleet
    /// agent's project input. A dry run reports the same and writes nothing.
    #[test]
    fn a_merge_carries_the_choices_named_by_owner_and_repo() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let old = s.upsert_project("frantisek", "app", "/p/app").unwrap();
        let new = s.upsert_project("sales-twins", "app", "/p/app").unwrap();
        let c = s.conn_for_test();
        // A pick under both names (the survivor's group wins) and one on
        // another repository of the old owner (left alone).
        for (o, r, grp) in [
            ("frantisek", "app", "old-grp"),
            ("sales-twins", "app", "new-grp"),
            ("frantisek", "lib", "lib-grp"),
        ] {
            c.execute(
                "INSERT INTO project_picks (owner, repo, pinned, grp, updated_at) \
                 VALUES (?1, ?2, 1, ?3, 1)",
                rusqlite::params![o, r, grp],
            )
            .unwrap();
        }
        // A workspace on the same worktree under both names (the survivor's
        // stays) and one only the old name has (it moves).
        for (o, key, local) in [
            ("frantisek", "main", "/l/old-main"),
            ("sales-twins", "main", "/l/new-main"),
            ("frantisek", "feat", "/l/old-feat"),
        ] {
            c.execute(
                "INSERT INTO local_workspaces (host_alias, owner, repo, worktree_key, \
                 remote_path, local_path, created_at) VALUES ('mac', ?1, 'app', ?2, '/r', ?3, 1)",
                rusqlite::params![o, key, local],
            )
            .unwrap();
        }
        for (scope, config) in [
            (
                "a",
                r#"{"repo":"frantisek/app","note":"frantisek/app is old"}"#,
            ),
            ("b", r#"{"repo":"frantisek/lib"}"#),
        ] {
            c.execute(
                "INSERT INTO agent_rules (agent, scope, level, host_alias, config, granted_by, \
                 granted_at, definition) VALUES ('fleet', ?1, 'watch', 'mac', ?2, 'me', 1, 'd')",
                rusqlite::params![scope, config],
            )
            .unwrap();
        }
        let picks = || {
            let mut st = c
                .prepare("SELECT owner || '/' || repo, grp FROM project_picks ORDER BY 1")
                .unwrap();
            let v = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap();
            v.collect::<Result<Vec<_>, _>>().unwrap()
        };
        let spaces = || {
            let mut st = c
                .prepare(
                    "SELECT owner || '/' || worktree_key, local_path FROM local_workspaces \
                     ORDER BY 1, 2",
                )
                .unwrap();
            let v = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap();
            v.collect::<Result<Vec<_>, _>>().unwrap()
        };
        let configs = || {
            let mut st = c
                .prepare("SELECT config FROM agent_rules ORDER BY scope")
                .unwrap();
            let v = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
            v.collect::<Result<Vec<_>, _>>().unwrap()
        };
        let before = (picks(), spaces(), configs());

        let dry = s.merge_project_with(old, new, true).unwrap();
        assert!(dry.dry_run);
        assert_eq!(
            (
                dry.picks_moved,
                dry.workspaces_moved,
                dry.workspaces_dropped,
                dry.agent_inputs_moved
            ),
            (0, 1, 1, 1)
        );
        assert_eq!(
            (picks(), spaces(), configs()),
            before,
            "a dry run writes nothing"
        );
        assert!(s.get_project(old).unwrap().is_some());

        let rep = s.merge_project(old, new).unwrap();
        assert!(!rep.dry_run);
        assert_eq!(
            (
                rep.picks_moved,
                rep.workspaces_moved,
                rep.workspaces_dropped,
                rep.agent_inputs_moved
            ),
            (
                dry.picks_moved,
                dry.workspaces_moved,
                dry.workspaces_dropped,
                dry.agent_inputs_moved
            )
        );
        assert_eq!(
            picks(),
            [
                ("frantisek/lib".to_string(), "lib-grp".to_string()),
                ("sales-twins/app".into(), "new-grp".into()),
            ],
            "the survivor's pick wins; another project's stays"
        );
        assert_eq!(
            spaces(),
            [
                ("sales-twins/feat".to_string(), "/l/old-feat".to_string()),
                ("sales-twins/main".into(), "/l/new-main".into()),
            ]
        );
        let cfg: Vec<serde_json::Value> = configs()
            .iter()
            .map(|t| serde_json::from_str(t).unwrap())
            .collect();
        assert_eq!(cfg[0]["repo"], "sales-twins/app");
        assert_eq!(
            cfg[0]["note"], "frantisek/app is old",
            "only an exact value"
        );
        assert_eq!(cfg[1]["repo"], "frantisek/lib");
    }

    fn account_row(s: &Store, uuid: &str) {
        s.upsert_account(&crate::store::AccountRow {
            uuid: uuid.into(),
            ..Default::default()
        })
        .unwrap();
    }

    /// Review w05-2: when the survivor already has a ticket space, the old
    /// one is left (its keys still resolve) and the report names it.
    #[test]
    fn a_merge_into_a_project_with_its_own_ticket_space_names_the_one_left() {
        let s = Store::open_in_memory().unwrap();
        let old = s.upsert_project("frantisek", "app", "/p/app").unwrap();
        let new = s.upsert_project("sales-twins", "app", "/p/app").unwrap();
        let left = s.project_ticket_space(old).unwrap();
        let own = s.project_ticket_space(new).unwrap();
        let rep = s.merge_project(old, new).unwrap();
        assert_eq!(rep.ticket_space_left, Some(left.prefix.clone()));
        assert_eq!(s.project_ticket_space(new).unwrap().id, own.id);
        assert!(s.ticket_space(left.id).unwrap().is_some());
    }

    /// Review w05-4: two rows over two checkouts are no merge: the next
    /// scan would split them again from the paths.
    /// Review w03-3: a review rule's `gh:<owner>/<repo>` scope and a
    /// routine's PR-event repo filter follow the merge; a review rule into
    /// already has for the new name wins.
    #[test]
    fn a_merge_moves_review_scopes_and_routine_repo_filters() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let old = s.upsert_project("frantisek", "app", "/p/app").unwrap();
        let new = s.upsert_project("sales-twins", "app", "/p/app").unwrap();
        let routine = s
            .insert_routine(
                None,
                None,
                &crate::store::RoutineFields {
                    name: "prs".into(),
                    enabled: true,
                    trigger: "manual".into(),
                    host_alias: "mac".into(),
                    project_id: old,
                    prompt: "p".into(),
                    overlap: "skip".into(),
                    event_repo: Some("Frantisek/App".into()),
                    ..Default::default()
                },
                None,
                0,
            )
            .unwrap();
        let rule = |scope: &str| {
            s.conn_for_test()
                .execute(
                    "INSERT INTO agent_rules (agent, scope, level, host_alias, granted_by, \
                     granted_at) VALUES ('review', ?1, 'watch', 'mac', 'me', 1)",
                    [scope],
                )
                .unwrap();
        };
        rule("gh:frantisek/app");
        let dry = s.merge_project_with(old, new, true).unwrap();
        assert_eq!((dry.review_rules_moved, dry.routine_filters_moved), (1, 1));
        // into's own rule for the new name wins over the moved one.
        rule("gh:sales-twins/app");
        let rep = s.merge_project(old, new).unwrap();
        assert_eq!((rep.review_rules_moved, rep.routine_filters_moved), (0, 1));
        let scopes: Vec<String> = {
            let c = s.conn_for_test();
            let mut st = c
                .prepare("SELECT scope FROM agent_rules WHERE agent = 'review'")
                .unwrap();
            let v = st.query_map([], |r| r.get(0)).unwrap();
            v.collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(scopes, vec!["gh:sales-twins/app".to_string()]);
        let repo: Option<String> = s
            .conn_for_test()
            .query_row(
                "SELECT event_repo FROM routines WHERE id = ?1",
                [routine.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repo.as_deref(), Some("sales-twins/app"));
    }

    #[test]
    fn a_merge_refuses_two_checkouts() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let old = s
            .upsert_project("frantisek", "app", "/proj/github.com/frantisek/app")
            .unwrap();
        let new = s
            .upsert_project("sales-twins", "app", "/proj/github.com/sales-twins/app")
            .unwrap();
        let e = s.merge_project(old, new).unwrap_err();
        assert_eq!(e.code, "E_INVALID", "{}", e.message);
        assert!(e.message.contains("old checkout"), "{}", e.message);
        // One base path, but a twin worktree in another directory.
        let new2 = s
            .upsert_project("acme", "app", "/proj/github.com/frantisek/app")
            .unwrap();
        let old_feat = s
            .upsert_worktree_on("mac", old, "feat", "/a/.worktrees/feat", None)
            .unwrap();
        let new_feat = s
            .upsert_worktree_on("mac", new2, "feat", "/b/.worktrees/feat", None)
            .unwrap();
        // A session sits on the old twin: it would carry the other
        // directory's row.
        let sid = s
            .upsert_session(
                "gone",
                "mac",
                Some(old),
                Some(old_feat),
                1,
                1,
                "ghost",
                None,
            )
            .unwrap();
        let e = s.merge_project(old, new2).unwrap_err();
        assert_eq!(e.code, "E_INVALID", "{}", e.message);
        assert!(e.message.contains("worktree feat"), "{}", e.message);
        assert!(e.message.contains("forget frantisek/app"), "{}", e.message);
        assert!(s.get_project(old).unwrap().is_some());
        // w01-6: once no session sits on it, a remote twin folds into
        // `into`'s row, as a twin in the same directory does.
        s.conn_for_test()
            .execute("DELETE FROM sessions WHERE id = ?1", [sid])
            .unwrap();
        s.merge_project(old, new2).unwrap();
        assert!(s.get_project(old).unwrap().is_none());
        let left: Vec<i64> = {
            let c = s.conn_for_test();
            let mut st = c
                .prepare("SELECT id FROM worktrees WHERE host_alias = 'mac' AND name = 'feat'")
                .unwrap();
            let v = st.query_map([], |r| r.get(0)).unwrap();
            v.collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(left, vec![new_feat]);
    }

    /// Review w05-7: a live session of the project that would go refuses
    /// the merge, as `forget_project` refuses it.
    #[test]
    fn a_merge_refuses_a_live_session_of_the_old_project() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let old = s.upsert_project("frantisek", "app", "/p/app").unwrap();
        let new = s.upsert_project("sales-twins", "app", "/p/app").unwrap();
        let sid = s
            .upsert_session("live", "mac", Some(old), None, 1, 1, "running", None)
            .unwrap();
        let e = s.merge_project(old, new).unwrap_err();
        assert_eq!(e.code, "E_INVALID_STATE", "{}", e.message);
        s.conn_for_test()
            .execute("UPDATE sessions SET status = 'ghost' WHERE id = ?1", [sid])
            .unwrap();
        s.merge_project(old, new).unwrap();
    }

    #[test]
    fn a_merge_refuses_what_is_not_two_repositories() {
        let s = Store::open_in_memory().unwrap();
        let a = s.upsert_project("o", "a", "/p/a").unwrap();
        let sys = s
            .upsert_system_project("fleet", "operator", "/home/me/.claude-fleet/operator")
            .unwrap();
        assert_eq!(s.merge_project(a, a).unwrap_err().code, "E_INVALID");
        assert_eq!(s.merge_project(a, 999).unwrap_err().code, "E_NOTFOUND");
        assert_eq!(s.merge_project(999, a).unwrap_err().code, "E_NOTFOUND");
        assert_eq!(s.merge_project(sys, a).unwrap_err().code, "E_INVALID");
        assert_eq!(s.merge_project(a, sys).unwrap_err().code, "E_INVALID");
        assert!(
            s.get_project(a).unwrap().is_some(),
            "a refusal changes nothing"
        );
    }

    /// data-sync F7: 12 duplicate worktree paths, linked worktrees scanned
    /// as repos, that a hub without a local host could never heal.
    #[test]
    fn dedupe_worktree_paths_keeps_the_owning_project_and_repoints_sessions() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let owner = s
            .upsert_project("o", "sales-twins-app", "/p/sales-twins-app")
            .unwrap();
        let dup = s
            .upsert_project(
                "o",
                "sal-696",
                "/p/sales-twins-app/.claude/worktrees/SAL-696",
            )
            .unwrap();
        let kept = s
            .upsert_worktree(
                owner,
                "SAL-696",
                "/p/sales-twins-app/.claude/worktrees/SAL-696",
                Some("SAL-696"),
            )
            .unwrap();
        let gone = s
            .upsert_worktree(
                dup,
                "main",
                "/p/sales-twins-app/.claude/worktrees/SAL-696",
                Some("SAL-696"),
            )
            .unwrap();
        let sid = s
            .upsert_session("dev", "local", Some(dup), Some(gone), 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.dedupe_worktree_paths().unwrap(), 1);
        assert!(s.get_worktree_row(gone).unwrap().is_none());
        assert!(s.get_worktree_row(kept).unwrap().is_some());
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.worktree_id, Some(kept));
        assert_eq!(row.project_id, Some(owner));
        assert_eq!(s.dedupe_worktree_paths().unwrap(), 0, "idempotent");
    }

    #[test]
    fn a_projects_ghost_rows_go_and_only_live_rows_count() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let live = s
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let ghost = s
            .upsert_session("b", "local", Some(pid), None, 1, 1, "ghost", None)
            .unwrap();
        assert_eq!(s.project_live_session_count(pid).unwrap(), 1);
        assert_eq!(s.delete_project_ghost_sessions(pid).unwrap(), vec![ghost]);
        assert!(s.get_session_by_id(ghost).unwrap().is_none());
        assert!(s.get_session_by_id(live).unwrap().is_some());
    }

    /// Callers holding only the mutex get the keys with the lock released
    /// before any path is resolved; a remote path is never resolved here.
    #[cfg(unix)]
    #[test]
    fn fingerprint_keys_are_resolved_outside_the_store_lock() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let (local, remote, pid) = {
            let s = store.lock().unwrap();
            s.upsert_host("vps").unwrap();
            let pid = s.upsert_project("o", "r", "/fleet-no-such/r").unwrap();
            let local = s
                .upsert_worktree(pid, "l", "/fleet-no-such/r/l", None)
                .unwrap();
            let remote = s
                .upsert_worktree_on("vps", pid, "v", "/srv/r/v", None)
                .unwrap();
            (local, remote, pid)
        };
        assert_eq!(
            Store::fingerprint_keys_of_worktree(&store, local),
            vec!["/fleet-no-such/r/l".to_string()]
        );
        assert_eq!(
            Store::fingerprint_keys_of_worktree(&store, remote),
            vec!["/srv/r/v".to_string()],
            "a remote path is never resolved on this machine"
        );
        assert!(store.try_lock().is_ok(), "the lock is released");
        // Local rows only: a remote row's key is its stored path, which the
        // delete always tries without precomputation.
        let by_project = Store::fingerprint_keys_of_project(&store, pid);
        assert_eq!(by_project.len(), 1, "{by_project:?}");
        assert!(by_project.contains_key(&local));
        assert!(Store::fingerprint_keys_of_worktree(&store, 999_999).is_empty());
    }

    #[test]
    fn upsert_and_list_projects_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .upsert_project("martin-janci", "claude-fleet", "/tmp/cf")
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_project("martin-janci", "claude-fleet", "/other/path")
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, "martin-janci");
        assert_eq!(rows[0].repo, "claude-fleet");
        assert_eq!(rows[0].base_path, "/other/path");
    }

    #[test]
    fn worktrees_upsert_list_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        s.upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        s.upsert_worktree(
            pid,
            "feature-x",
            "/tmp/r/.worktrees/feature-x",
            Some("feature-x"),
        )
        .unwrap();
        s.upsert_worktree(pid, "bugfix", "/tmp/r/.worktrees/bugfix", Some("bugfix"))
            .unwrap();
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 3);
        let removed = s
            .delete_worktrees_not_in(
                pid,
                &["main".to_string(), "feature-x".to_string()],
                |p: &str| p.to_string(),
                &FingerprintKeys::new(),
            )
            .unwrap();
        assert_eq!(removed, 1);
        let names: Vec<String> = s
            .list_worktrees_for_project(pid)
            .unwrap()
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, vec!["feature-x", "main"]);
    }

    #[test]
    fn delete_worktrees_not_in_repoints_sessions_to_the_same_checkout() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        // The old basename-named main row (logical spelling) and a worktree
        // that no longer exists, each referenced by a session.
        let old = s.upsert_worktree(pid, "r", "/tmp/link/r", None).unwrap();
        let gone = s
            .upsert_worktree(pid, "gone", "/tmp/r/.worktrees/gone", None)
            .unwrap();
        let on_old = s
            .upsert_session("dev", "local", Some(pid), Some(old), 1, 1, "running", None)
            .unwrap();
        let on_gone = s
            .upsert_session(
                "dev2",
                "local",
                Some(pid),
                Some(gone),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let main = s
            .upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        // `/tmp/link/r` is another spelling of `/tmp/r`.
        let canon = |p: &str| p.replace("/tmp/link/", "/tmp/");
        let removed = s
            .delete_worktrees_not_in(pid, &["main".to_string()], canon, &FingerprintKeys::new())
            .expect("referenced rows must not fail the foreign key");
        assert_eq!(removed, 2);
        assert_eq!(
            s.get_session_by_id(on_old).unwrap().unwrap().worktree_id,
            Some(main),
            "moved to the surviving row of the same checkout"
        );
        assert_eq!(
            s.get_session_by_id(on_gone).unwrap().unwrap().worktree_id,
            None,
            "no survivor: the reference is cleared"
        );
    }

    #[test]
    fn delete_project_if_unused_counts_worktree_references_from_other_projects() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // The duplicate-scan bug: a session of ANOTHER project points at a
        // worktree row of `dup`.
        let dup = s.upsert_project("o", "dup", "/tmp/dup").unwrap();
        let w = s.upsert_worktree(dup, "main", "/tmp/dup", None).unwrap();
        let real = s.upsert_project("o", "real", "/tmp/real").unwrap();
        s.upsert_session("dev", "local", Some(real), Some(w), 1, 1, "running", None)
            .unwrap();
        assert!(
            !s.delete_project_if_unused(dup, &FingerprintKeys::new())
                .unwrap(),
            "still referenced"
        );
        assert!(s.get_worktree_row(w).unwrap().is_some());
        let free = s.upsert_project("o", "free", "/tmp/free").unwrap();
        s.upsert_worktree(free, "main", "/tmp/free", None).unwrap();
        assert!(s
            .delete_project_if_unused(free, &FingerprintKeys::new())
            .unwrap());
    }

    #[test]
    fn delete_project_forgets_the_work_links_snapshot_of_it() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/fleet-test/r").unwrap();
        let other = s.upsert_project("o", "k", "/fleet-test/k").unwrap();
        for p in [pid, other] {
            s.conn
                .execute(
                    "INSERT INTO work_links (ref_key, state, source, created_at, snap_project_id) \
                     VALUES ('ABC-1', 'ended', 'test', 1, ?1)",
                    rusqlite::params![p],
                )
                .unwrap();
        }
        s.delete_project(pid, &FingerprintKeys::new()).unwrap();
        let snaps: Vec<Option<i64>> = s
            .conn
            .prepare("SELECT snap_project_id FROM work_links ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(snaps, vec![None, Some(other)]);
    }

    #[test]
    fn delete_project_reaps_timeline_and_worktree_fingerprints_but_tombstones_the_inbox() {
        // Previously named `..._reaps_timeline_inbox_and_...` and asserted
        // `list_inbox(sid, ...)` was empty after the delete — that
        // assertion encoded the pre-existing defect Task 13 fixes (deleting
        // a project used to destroy every undelivered message addressed to
        // its sessions). The message now survives; only the participant
        // identity is tombstoned.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/fleet-test/r").unwrap();
        let other_pid = s.upsert_project("o", "k", "/fleet-test/k").unwrap();
        s.upsert_worktree(pid, "feat", "/fleet-test/r/.worktrees/feat", Some("feat"))
            .unwrap();
        s.upsert_worktree(
            other_pid,
            "keep",
            "/fleet-test/k/.worktrees/keep",
            Some("keep"),
        )
        .unwrap();
        let sid = s
            .upsert_session("dev-r", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let peer = s
            .upsert_session(
                "peer",
                "local",
                Some(other_pid),
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.insert_session_event(sid, "status_change", Some("idle"))
            .unwrap();
        let to_project = s
            .insert_message(peer, sid, "to the project", "message", None)
            .unwrap();
        let participant = s.participant_for_session(sid).unwrap().unwrap().id;
        s.record_parent_fingerprint("local", "/fleet-test/r/.worktrees/feat", "1:2", 1)
            .unwrap();
        s.record_parent_fingerprint("local", "/fleet-test/k/.worktrees/keep", "3:4", 1)
            .unwrap();

        s.delete_project(pid, &FingerprintKeys::new()).unwrap();

        assert!(s.list_session_events(sid, 10).unwrap().is_empty());
        assert!(
            s.get_message(to_project).unwrap().is_some(),
            "deleting a project must not destroy undelivered mail"
        );
        let p = s.participant_by_id(participant).unwrap().unwrap();
        assert!(p.retired_at.is_some(), "the identity is tombstoned instead");
        assert_eq!(
            s.parent_fingerprint("local", "/fleet-test/r/.worktrees/feat")
                .unwrap(),
            None
        );
        assert_eq!(
            s.parent_fingerprint("local", "/fleet-test/k/.worktrees/keep")
                .unwrap()
                .as_deref(),
            Some("3:4")
        );
    }

    /// The canonical key is resolved by the CALLER, off-lock
    /// (`Store::fingerprint_keys`), and handed to the delete functions; the
    /// store itself never touches the filesystem, so a delete without keys
    /// removes only the stored path.
    #[cfg(unix)]
    #[test]
    fn deleting_worktree_rows_drops_their_fingerprints() {
        let s = Store::open_in_memory().unwrap();
        let base = tempfile::TempDir::new().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Rows keep the symlinked spelling; the probe records the canonical one.
        let root_s = format!("{}/r", link.to_str().unwrap());
        let pid = s.upsert_project("o", "r", &root_s).unwrap();
        let w1_path = format!("{root_s}/.worktrees/w1");
        let w3_path = format!("{root_s}/.worktrees/w3");
        let w1 = s.upsert_worktree(pid, "w1", &w1_path, Some("w1")).unwrap();
        let w3 = s.upsert_worktree(pid, "w3", &w3_path, Some("w3")).unwrap();
        s.upsert_worktree(pid, "w2", &format!("{root_s}/.worktrees/w2"), Some("w2"))
            .unwrap();
        let w1_keys = Store::fingerprint_keys("local", &w1_path);
        let w3_keys = Store::fingerprint_keys("local", &w3_path);
        assert_eq!(
            w1_keys.len(),
            2,
            "the canonical spelling differs: {w1_keys:?}"
        );
        let (w1_canon, w3_canon) = (w1_keys[1].clone(), w3_keys[1].clone());
        s.record_parent_fingerprint("local", &w1_canon, "1:1", 1)
            .unwrap();
        s.record_parent_fingerprint("local", &w3_canon, "3:3", 1)
            .unwrap();
        s.record_parent_fingerprint("local", &format!("{root_s}/.worktrees/w2"), "2:2", 1)
            .unwrap();
        s.record_parent_fingerprint("local", "/elsewhere/w", "9:9", 1)
            .unwrap();

        // The precomputed keys reach the delete: the canonical key goes.
        s.delete_worktree(w1, &w1_keys).unwrap();
        assert_eq!(s.parent_fingerprint("local", &w1_canon).unwrap(), None);
        // Without keys the store resolves nothing itself.
        s.delete_worktree(w3, &[]).unwrap();
        assert_eq!(
            s.parent_fingerprint("local", &w3_canon).unwrap().as_deref(),
            Some("3:3"),
            "the store never canonicalizes under its lock"
        );

        let rows = s.list_worktrees_for_project(pid).unwrap();
        s.delete_worktrees_not_in(
            pid,
            &[],
            |p| p.to_string(),
            &Store::fingerprint_keys_for(&rows),
        )
        .unwrap();
        assert_eq!(
            s.parent_fingerprint("local", &format!("{root_s}/.worktrees/w2"))
                .unwrap(),
            None
        );
        assert_eq!(
            s.parent_fingerprint("local", "/elsewhere/w")
                .unwrap()
                .as_deref(),
            Some("9:9")
        );
    }

    #[test]
    fn list_projects_joined_groups_worktrees_by_project() {
        let s = Store::open_in_memory().expect("store");
        s.upsert_project("o1", "r1", "/p1").unwrap();
        s.upsert_project("o2", "r2", "/p2").unwrap();
        s.upsert_worktree(1, "main", "/p1", None).unwrap();
        s.upsert_worktree(1, "feature", "/p1/.worktrees/feature", Some("feature"))
            .unwrap();
        s.upsert_worktree(2, "main", "/p2", None).unwrap();
        let trees = s.list_projects_joined().expect("joined");
        assert_eq!(trees.len(), 2);
        let p1 = trees.iter().find(|t| t.project.repo == "r1").expect("p1");
        let p2 = trees.iter().find(|t| t.project.repo == "r2").expect("p2");
        assert_eq!(p1.worktrees.len(), 2);
        assert_eq!(p2.worktrees.len(), 1);
    }

    /// Worktree rows are host-scoped (024) and fingerprints are keyed by
    /// (host, path): deleting a REMOTE row drops that host's fingerprint by
    /// its stored path, never resolving it on this machine, and leaves a
    /// local fingerprint with the same path untouched. Through
    /// `delete_worktree` and through `delete_project`.
    #[test]
    fn deleting_a_remote_worktree_row_drops_only_that_hosts_fingerprint() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let path = "/srv/r/.worktrees/feat";
        let remote = s
            .upsert_worktree_on("vps", pid, "feat", path, None)
            .unwrap();
        s.record_parent_fingerprint("vps", path, "9:9", 1).unwrap();
        s.record_parent_fingerprint("local", path, "1:1", 1)
            .unwrap();
        s.delete_worktree(remote, &Store::fingerprint_keys("vps", path))
            .unwrap();
        assert_eq!(s.parent_fingerprint("vps", path).unwrap(), None);
        assert_eq!(
            s.parent_fingerprint("local", path).unwrap().as_deref(),
            Some("1:1"),
            "a local fingerprint with the same path is not the remote row's"
        );

        let other = "/srv/r/.worktrees/other";
        s.upsert_worktree_on("vps", pid, "other", other, None)
            .unwrap();
        s.record_parent_fingerprint("vps", other, "9:8", 1).unwrap();
        s.record_parent_fingerprint("local", other, "1:2", 1)
            .unwrap();
        s.delete_project(pid, &FingerprintKeys::new()).unwrap();
        assert_eq!(s.parent_fingerprint("vps", other).unwrap(), None);
        assert_eq!(
            s.parent_fingerprint("local", other).unwrap().as_deref(),
            Some("1:2")
        );
    }

    #[test]
    fn delete_worktrees_at_emits_session_updated_for_cleared_sessions() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let wt = s
            .upsert_worktree_on("vps", pid, "feat", "/home/u/r/.worktrees/feat", None)
            .unwrap();
        let sid = s
            .upsert_session("dev", "vps", Some(pid), Some(wt), 1, 1, "running", None)
            .unwrap();
        bus.take();
        assert_eq!(
            s.delete_worktrees_at("vps", "/home/u/r/.worktrees/feat")
                .unwrap(),
            1
        );
        let evts = bus.take();
        assert!(
            evts.contains(&format!("session:updated:{sid}")),
            "the sidebar learns the worktree is gone: {evts:?}"
        );
        assert_eq!(s.get_session_by_id(sid).unwrap().unwrap().worktree_id, None);
    }

    #[test]
    fn delete_worktrees_not_in_emits_session_updated_for_moved_sessions() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        let old = s.upsert_worktree(pid, "r", "/tmp/r", None).unwrap();
        let sid = s
            .upsert_session("dev", "local", Some(pid), Some(old), 1, 1, "running", None)
            .unwrap();
        let main = s
            .upsert_worktree(pid, "main", "/tmp/r", Some("main"))
            .unwrap();
        bus.take();
        s.delete_worktrees_not_in(
            pid,
            &["main".to_string()],
            |p: &str| p.to_string(),
            &FingerprintKeys::new(),
        )
        .unwrap();
        let evts = bus.take();
        assert!(
            evts.contains(&format!("session:updated:{sid}")),
            "the sidebar learns of the re-point at once: {evts:?}"
        );
        assert_eq!(
            s.get_session_by_id(sid).unwrap().unwrap().worktree_id,
            Some(main)
        );
    }

    #[test]
    fn delete_host_worktrees_not_in_prunes_only_that_hosts_unlisted_rows() {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("vps").unwrap();
        s.upsert_host("vps2").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let local = s
            .upsert_worktree(pid, "feat", "/p/o/r/.claude/worktrees/feat", Some("feat"))
            .unwrap();
        let keep = s
            .upsert_worktree_on("vps", pid, "main", "/home/u/r", Some("main"))
            .unwrap();
        let gone = s
            .upsert_worktree_on("vps", pid, "old", "/home/u/r/.claude/worktrees/old", None)
            .unwrap();
        let busy = s
            .upsert_worktree_on("vps", pid, "busy", "/home/u/r/.claude/worktrees/busy", None)
            .unwrap();
        let other_host = s
            .upsert_worktree_on(
                "vps2",
                pid,
                "stale",
                "/home/u2/r/.claude/worktrees/stale",
                None,
            )
            .unwrap();
        // An alive session pins `busy` even though the scan did not list it.
        s.upsert_session(
            "dev-r--busy",
            "vps",
            Some(pid),
            Some(busy),
            1,
            1,
            "running",
            None,
        )
        .unwrap();

        // Local rows are never touched, even with an empty keep list.
        assert_eq!(
            s.delete_host_worktrees_not_in("local", pid, &[]).unwrap(),
            0
        );
        assert!(s.get_worktree_row(local).unwrap().is_some());

        // An empty keep list means the scan failed (a successful scan always
        // reports the root worktree) — nothing is pruned.
        assert_eq!(s.delete_host_worktrees_not_in("vps", pid, &[]).unwrap(), 0);
        assert!(s.get_worktree_row(gone).unwrap().is_some());

        bus.take();
        let removed = s
            .delete_host_worktrees_not_in("vps", pid, &["main".to_string()])
            .unwrap();

        assert_eq!(removed, 1);
        assert!(s.get_worktree_row(gone).unwrap().is_none());
        assert!(s.get_worktree_row(keep).unwrap().is_some());
        assert!(
            s.get_worktree_row(busy).unwrap().is_some(),
            "row with an alive session is kept"
        );
        assert!(
            s.get_worktree_row(local).unwrap().is_some(),
            "local rows are never touched"
        );
        assert!(
            s.get_worktree_row(other_host).unwrap().is_some(),
            "another host's unlisted row is untouched by this host's prune"
        );
        let evts = bus.take();
        assert_eq!(
            evts.iter()
                .filter(|e| *e == &format!("worktree:removed:{gone}"))
                .count(),
            1,
            "worktree:removed fires once for the pruned row: {evts:?}"
        );
    }

    /// `worktree_occupancy` replaces `list_worktrees`'s old per-project +
    /// per-worktree N+1 with one query; this pins that its output matches
    /// what the N+1 path produced (project order, then worktree name order,
    /// then occupant host/name order), across several projects and
    /// worktrees, with a mix of an occupied worktree, a free worktree, and a
    /// worktree whose only session is LOST (must not count as an occupant).
    #[test]
    fn worktree_occupancy_matches_the_old_n_plus_1_shape() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("alpha").unwrap();
        s.upsert_host("beta").unwrap();

        // Two projects, deliberately upserted out of (owner, repo) order, to
        // prove the query orders by project like `list_projects` does.
        let pid_z = s.upsert_project("z-owner", "repo", "/p/z").unwrap();
        let pid_a = s.upsert_project("a-owner", "repo", "/p/a").unwrap();

        // pid_a: two worktrees, "main" and "feat"; "feat" has two alive
        // occupants on different hosts, "main" is free.
        let wid_a_main = s
            .upsert_worktree(pid_a, "main", "/p/a", Some("main"))
            .unwrap();
        let wid_a_feat = s
            .upsert_worktree(pid_a, "feat", "/p/a/.worktrees/feat", Some("feat"))
            .unwrap();
        s.upsert_session(
            "sess-beta",
            "beta",
            Some(pid_a),
            Some(wid_a_feat),
            0,
            0,
            "running",
            None,
        )
        .unwrap();
        s.upsert_session(
            "sess-alpha",
            "alpha",
            Some(pid_a),
            Some(wid_a_feat),
            0,
            0,
            "running",
            None,
        )
        .unwrap();

        // pid_z: one worktree whose only session is LOST — must read as
        // unoccupied, not as an occupant.
        let wid_z = s
            .upsert_worktree(pid_z, "gone", "/p/z/.worktrees/gone", None)
            .unwrap();
        let lost_id = s
            .upsert_session(
                "sess-lost",
                "alpha",
                Some(pid_z),
                Some(wid_z),
                0,
                0,
                "running",
                None,
            )
            .unwrap();
        s.conn
            .execute(
                "UPDATE sessions SET lost_at = 999 WHERE id = ?1",
                rusqlite::params![lost_id],
            )
            .unwrap();

        type Occupant = (String, String);
        type Shape = (String, i64, Vec<Occupant>);

        let out = s.worktree_occupancy(None).unwrap();
        let shape: Vec<Shape> = out
            .iter()
            .map(|o| {
                (
                    o.worktree.name.clone(),
                    o.worktree.id,
                    o.occupants
                        .iter()
                        .map(|occ| (occ.host_alias.clone(), occ.tmux_name.clone()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                // a-owner/repo first (project order); within it, alphabetical
                // by name ("feat" < "main"), matching
                // `list_worktrees_for_project`'s own `ORDER BY name` — the
                // old N+1 path never special-cased "main" first.
                (
                    "feat".to_string(),
                    wid_a_feat,
                    vec![
                        ("alpha".to_string(), "sess-alpha".to_string()),
                        ("beta".to_string(), "sess-beta".to_string()),
                    ]
                ),
                ("main".to_string(), wid_a_main, vec![]),
                // z-owner/repo last; its only session is lost, so no occupant
                ("gone".to_string(), wid_z, vec![]),
            ]
        );

        // project_id filters to just that project's worktrees, same as the
        // old per-project loop skipping every other project.
        let scoped = s.worktree_occupancy(Some(pid_a)).unwrap();
        assert_eq!(scoped.len(), 2);
        assert!(scoped.iter().all(|o| o.worktree.project_id == pid_a));

        // A project id that names no project answers empty, not an error —
        // exactly the old loop's behaviour when nothing matches.
        assert!(s.worktree_occupancy(Some(999_999)).unwrap().is_empty());
    }

    /// Migration 056's partial index on `sessions(worktree_id)` must be the
    /// one the planner picks for `worktree_occupancy`'s join — the whole
    /// point of adding it. `EXPLAIN QUERY PLAN`'s `detail` column names the
    /// index by name when it is used.
    #[test]
    fn worktree_occupancy_query_plan_uses_the_worktree_index() {
        let s = Store::open_in_memory().unwrap();
        let mut stmt = s
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT w.id FROM worktrees w
                 JOIN projects p ON p.id = w.project_id
            LEFT JOIN sessions s ON s.worktree_id = w.id
                                 AND s.status = 'running' AND s.lost_at IS NULL
                WHERE w.host_alias = 'local'
                ORDER BY p.owner, p.repo, w.name, s.host_alias, s.tmux_name",
            )
            .unwrap();
        let details: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|d| d.contains("idx_sessions_worktree_live")),
            "expected idx_sessions_worktree_live in the plan: {details:?}"
        );
    }
}
