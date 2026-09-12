//! Projects, worktrees and worktree parent fingerprints.

use super::*;

/// Ids of the sessions whose `worktree_id` is `worktree_id`: the rows a
/// re-point or clear is about to change, for the `session:updated` events
/// emitted after the commit.
fn session_ids_on_worktree(conn: &Connection, worktree_id: i64) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM sessions WHERE worktree_id=?1")?;
    let rows = stmt.query_map(rusqlite::params![worktree_id], |r| r.get(0))?;
    rows.collect()
}

impl Store {
    fn get_project(&self, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
        fetch_project(&self.conn, id)
    }

    fn get_worktree(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {WORKTREE_COLUMNS} FROM worktrees WHERE id=?1"
        ))?;
        stmt.query_row(rusqlite::params![id], worktree_from_row)
            .optional()
    }

    // ---- Public mutation methods ----

    pub fn upsert_project(
        &self,
        owner: &str,
        repo: &str,
        base_path: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO projects (owner, repo, base_path) VALUES (?1, ?2, ?3)
             ON CONFLICT(owner, repo) DO UPDATE SET base_path=excluded.base_path",
            rusqlite::params![owner, repo, base_path],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE owner=?1 AND repo=?2",
            rusqlite::params![owner, repo],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_project(id)? {
            self.bus.project_updated(&row);
        }
        Ok(id)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, owner, repo, base_path, last_session_at FROM projects ORDER BY owner, repo",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                owner: row.get(1)?,
                repo: row.get(2)?,
                base_path: row.get(3)?,
                last_session_at: row.get(4)?,
            })
        })?;
        rows.collect()
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
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.owner, p.repo, p.base_path, p.last_session_at,
                    w.id, w.project_id, w.name, w.path, w.branch
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id AND w.host_alias = 'local'
             ORDER BY
               CASE WHEN p.last_session_at IS NULL THEN 1 ELSE 0 END,
               p.last_session_at DESC,
               p.id,
               w.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;
        let mut out: Vec<crate::service::projects::ProjectTreeRow> = Vec::new();
        let mut last_pid: Option<i64> = None;
        for r in rows {
            let (pid, owner, repo, base, last, wid, _wpid, wname, wpath, wbranch) = r?;
            if last_pid != Some(pid) {
                out.push(crate::service::projects::ProjectTreeRow {
                    project: ProjectRow {
                        id: pid,
                        owner,
                        repo,
                        base_path: base,
                        last_session_at: last,
                    },
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
        self.conn.execute(
            "INSERT INTO worktrees (project_id, host_alias, name, path, branch, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(project_id, host_alias, name)
             DO UPDATE SET path=excluded.path, branch=excluded.branch,
                           updated_at_ms=excluded.updated_at_ms",
            rusqlite::params![project_id, host_alias, name, path, branch, now_ms],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM worktrees WHERE project_id=?1 AND host_alias=?2 AND name=?3",
            rusqlite::params![project_id, host_alias, name],
            |row| row.get(0),
        )?;
        if host_alias == crate::service::projects::LOCAL_HOST {
            if let Some(row) = self.get_worktree(id)? {
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
        let Some(row) = self.get_worktree(id)? else {
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
            Ok(s) => s.get_worktree(id).ok().flatten(),
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

    pub fn get_worktree_row(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        self.get_worktree(id)
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
            if let Ok(Some(row)) = self.get_session_by_id(*id) {
                self.bus.session_updated(&row);
            }
        }
    }

    pub fn touch_project_last_session_at(
        &self,
        project_id: i64,
        ts: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE projects SET last_session_at = MAX(COALESCE(last_session_at, 0), ?1) WHERE id = ?2",
            rusqlite::params![ts, project_id],
        )?;
        if let Some(row) = self.get_project(project_id)? {
            self.bus.project_updated(&row);
        }
        Ok(())
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
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(crate::ipc_error::IpcError::from)?;
        // What dies with the sessions (as `delete_session` does): their
        // timeline and the messages addressed to them. And the recorded
        // parent fingerprints (repair) of the project's worktree rows, each
        // under the ROW's host (rows are host-scoped, migration 024).
        tx.execute(
            "DELETE FROM session_events
              WHERE session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
            rusqlite::params![project_id],
        )?;
        tx.execute(
            "DELETE FROM session_messages
              WHERE to_session_id IN (SELECT id FROM sessions WHERE project_id = ?1)",
            rusqlite::params![project_id],
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
        tx.execute(
            "DELETE FROM sessions WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.execute(
            "DELETE FROM worktrees WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(crate::ipc_error::IpcError::from)?;
        tx.commit().map_err(crate::ipc_error::IpcError::from)?;
        self.emit_sessions_updated(&cross);
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
        use rusqlite::OptionalExtension;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Callers holding only the mutex get the keys with the lock released
    /// before any path is resolved; a remote path is never resolved here.
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
    fn touch_project_last_session_at_takes_max() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        // First write
        s.touch_project_last_session_at(pid, 1000).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(1000));
        // Earlier timestamp shouldn't go backward
        s.touch_project_last_session_at(pid, 500).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(1000));
        // Later timestamp wins
        s.touch_project_last_session_at(pid, 2000).unwrap();
        let rows = s.list_projects().unwrap();
        assert_eq!(rows[0].last_session_at, Some(2000));
    }

    #[test]
    fn delete_project_reaps_timeline_inbox_and_worktree_fingerprints() {
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
        s.insert_message(peer, sid, "to the project", "message", None)
            .unwrap();
        s.record_parent_fingerprint("local", "/fleet-test/r/.worktrees/feat", "1:2", 1)
            .unwrap();
        s.record_parent_fingerprint("local", "/fleet-test/k/.worktrees/keep", "3:4", 1)
            .unwrap();

        s.delete_project(pid, &FingerprintKeys::new()).unwrap();

        assert!(s.list_session_events(sid, 10).unwrap().is_empty());
        assert!(s.list_inbox(sid, false, 10).unwrap().is_empty());
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
}
