//! Local workspace sync (migration 109,
//! `docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md`): the
//! link between one remote worktree and one directory on this machine, its
//! BASE (what both sides last agreed on, per path) and its open conflicts.
//! Phases 2 and 3 (migration 112,
//! `docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md`)
//! add who drives the worktree and the per-path activity log: what each
//! side changed that nobody has looked at yet.
//! The engine is `service::local_sync`; nothing here touches a file.

use super::*;
use crate::ipc_error::{codes, IpcError};
use std::collections::HashMap;

/// The values `local_workspaces.state` takes.
pub const LOCAL_WORKSPACE_STATES: [&str; 8] = [
    "synced",
    "local_changes",
    "remote_changes",
    "syncing",
    "conflict",
    "paused",
    "offline",
    "error",
];

/// The values `local_workspace_conflicts.kind` takes (the migration's CHECK).
pub const LOCAL_CONFLICT_KINDS: [&str; 4] = [
    "both_modified",
    "both_added",
    "local_deleted",
    "remote_deleted",
];

/// The values `local_workspaces.driver` takes (migration 112's CHECK).
pub const LOCAL_WORKSPACE_DRIVERS: [&str; 3] = ["shared", "developer", "agent"];

/// One link, with its open conflicts. `project_id` is the current id of the
/// owner/repo project (ids are re-derived, so it is joined, never stored);
/// `None` when the project is gone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalWorkspaceRow {
    pub id: i64,
    pub host_alias: String,
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub project_id: Option<i64>,
    pub worktree_key: String,
    pub remote_path: String,
    pub local_path: String,
    #[serde(default)]
    pub session_id: Option<i64>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub excludes: Vec<String>,
    pub state: String,
    #[serde(default)]
    pub last_sync_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub pending_local: i64,
    #[serde(default)]
    pub pending_remote: i64,
    /// Paths left out on purpose this pass (symlinks, files over the size cap).
    #[serde(default)]
    pub skipped: i64,
    #[serde(default)]
    pub conflicts: Vec<LocalConflictRow>,
    pub created_at: i64,
    /// Who drives the worktree: `shared`, `developer` or `agent`.
    #[serde(default = "shared")]
    pub driver: String,
    /// When `driver` was last set.
    #[serde(default)]
    pub driver_since: Option<i64>,
    /// Changes carried from the folder to the host that nobody has handed
    /// on, committed, discarded or dismissed yet (activity rows, `local`).
    #[serde(default)]
    pub local_activity: i64,
    /// The same for changes carried from the host to the folder (`remote`).
    #[serde(default)]
    pub remote_activity: i64,
}

fn shared() -> String {
    "shared".to_string()
}

/// One path a pass carried and nobody has looked at yet.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalActivityRow {
    pub path: String,
    /// `local` (the folder's change, pushed) or `remote` (the host's, pulled).
    pub origin: String,
    /// `added`, `modified` or `deleted`.
    pub change: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalConflictRow {
    pub path: String,
    pub kind: String,
    pub detected_at: i64,
    /// `local` / `remote` once the person picked a side; the next pass
    /// carries it out (or, when that side moved again, asks anew).
    #[serde(default)]
    pub resolution: Option<String>,
    /// Each side as last seen; `None` = absent there. Engine state, not
    /// shown, so never on the wire.
    #[serde(skip)]
    pub local: Option<SideSeen>,
    #[serde(skip)]
    pub remote: Option<SideSeen>,
}

/// One side of a conflicting path: its content hash and stat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideSeen {
    pub sha256: String,
    pub stat: FileStat,
}

/// A conflict a pass found (or saw move): replaces the path's row and
/// clears any resolution.
#[derive(Debug, Clone)]
pub struct NewLocalConflict {
    pub path: String,
    pub kind: &'static str,
    pub local: Option<SideSeen>,
    pub remote: Option<SideSeen>,
}

/// What a new link starts from.
pub struct NewLocalWorkspace<'a> {
    pub host_alias: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub worktree_key: &'a str,
    pub remote_path: &'a str,
    pub local_path: &'a str,
    pub session_id: Option<i64>,
    pub excludes: &'a [String],
}

/// One side's stat of a file: size in bytes and mtime (local: nanoseconds,
/// remote: seconds — each side is only ever compared with itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub size: i64,
    pub mtime: i64,
}

/// One BASE entry. A side's stat is `None` when it must be re-checked
/// (recorded too close to the file's own mtime, or never seen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseEntry {
    pub sha256: String,
    pub local: Option<FileStat>,
    pub remote: Option<FileStat>,
}

/// The status columns a pass writes.
#[derive(Debug, Clone, Default)]
pub struct LocalWorkspaceStatus<'a> {
    pub state: &'a str,
    pub last_error: Option<&'a str>,
    pub pending_local: i64,
    pub pending_remote: i64,
    pub skipped: i64,
    /// Set `last_sync_at` to this (a completed pass); `None` leaves it.
    pub synced_at: Option<i64>,
}

/// Everything one pass learned, written in one transaction.
#[derive(Debug, Default)]
pub struct LocalPassWrite {
    pub upsert: Vec<(String, BaseEntry)>,
    pub remove: Vec<String>,
    pub add_conflicts: Vec<NewLocalConflict>,
    pub clear_conflicts: Vec<String>,
    /// What was carried across, for the activity log: path, origin
    /// (`local` / `remote`) and change (`added` / `modified` / `deleted`).
    pub activity: Vec<(String, &'static str, &'static str)>,
}

const COLS: &str = "w.id, w.host_alias, w.owner, w.repo,
       (SELECT p.id FROM projects p WHERE p.owner = w.owner AND p.repo = w.repo
         ORDER BY p.system, p.id LIMIT 1),
       w.worktree_key, w.remote_path, w.local_path, w.session_id, w.paused, w.excludes,
       w.state, w.last_sync_at, w.last_error, w.pending_local, w.pending_remote, w.skipped,
       w.created_at, w.driver, w.driver_since,
       (SELECT COUNT(*) FROM local_workspace_activity a
         WHERE a.workspace_id = w.id AND a.origin = 'local'),
       (SELECT COUNT(*) FROM local_workspace_activity a
         WHERE a.workspace_id = w.id AND a.origin = 'remote')";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalWorkspaceRow> {
    let excludes: String = r.get(10)?;
    Ok(LocalWorkspaceRow {
        id: r.get(0)?,
        host_alias: r.get(1)?,
        owner: r.get(2)?,
        repo: r.get(3)?,
        project_id: r.get(4)?,
        worktree_key: r.get(5)?,
        remote_path: r.get(6)?,
        local_path: r.get(7)?,
        session_id: r.get(8)?,
        paused: r.get::<_, i64>(9)? != 0,
        excludes: serde_json::from_str(&excludes).unwrap_or_default(),
        state: r.get(11)?,
        last_sync_at: r.get(12)?,
        last_error: r.get(13)?,
        pending_local: r.get(14)?,
        pending_remote: r.get(15)?,
        skipped: r.get(16)?,
        conflicts: Vec::new(),
        created_at: r.get(17)?,
        driver: r.get(18)?,
        driver_since: r.get(19)?,
        local_activity: r.get(20)?,
        remote_activity: r.get(21)?,
    })
}

fn stat_of(size: Option<i64>, mtime: Option<i64>) -> Option<FileStat> {
    match (size, mtime) {
        (Some(size), Some(mtime)) => Some(FileStat { size, mtime }),
        _ => None,
    }
}

fn not_found(id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("no local workspace {id}"))
}

impl Store {
    fn local_workspace_changed(&self, id: i64) {
        self.bus.emit(&RowChange::LocalWorkspaceChanged(id));
    }

    /// Every link, by id, each with its open conflicts.
    pub fn list_local_workspaces(&self) -> Result<Vec<LocalWorkspaceRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM local_workspaces w ORDER BY w.id"
        ))?;
        let mut rows = stmt
            .query_map([], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut conflicts = self.local_conflicts(None)?;
        for row in &mut rows {
            row.conflicts = conflicts.remove(&row.id).unwrap_or_default();
        }
        Ok(rows)
    }

    /// One link with its open conflicts, or `None`.
    pub fn local_workspace(&self, id: i64) -> Result<Option<LocalWorkspaceRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM local_workspaces w WHERE w.id = ?1"
        ))?;
        let Some(mut row) = stmt.query_row([id], map_row).optional()? else {
            return Ok(None);
        };
        row.conflicts = self
            .local_conflicts(Some(id))?
            .remove(&id)
            .unwrap_or_default();
        Ok(Some(row))
    }

    fn local_conflicts(
        &self,
        id: Option<i64>,
    ) -> Result<HashMap<i64, Vec<LocalConflictRow>>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT workspace_id, path, kind, detected_at, resolution,
                    local_sha, local_size, local_mtime, remote_sha, remote_size, remote_mtime
               FROM local_workspace_conflicts
              WHERE ?1 IS NULL OR workspace_id = ?1 ORDER BY workspace_id, path",
        )?;
        fn side(sha: Option<String>, size: Option<i64>, mtime: Option<i64>) -> Option<SideSeen> {
            Some(SideSeen {
                sha256: sha?,
                stat: stat_of(size, mtime)?,
            })
        }
        let mut out: HashMap<i64, Vec<LocalConflictRow>> = HashMap::new();
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                LocalConflictRow {
                    path: r.get(1)?,
                    kind: r.get(2)?,
                    detected_at: r.get(3)?,
                    resolution: r.get(4)?,
                    local: side(r.get(5)?, r.get(6)?, r.get(7)?),
                    remote: side(r.get(8)?, r.get(9)?, r.get(10)?),
                },
            ))
        })?;
        for row in rows {
            let (wid, c) = row?;
            out.entry(wid).or_default().push(c);
        }
        Ok(out)
    }

    /// Make a link. `E_CONFLICT` when the worktree already has one, or the
    /// directory is already some link's (or inside / around one).
    pub fn insert_local_workspace(
        &self,
        w: &NewLocalWorkspace<'_>,
        now: i64,
    ) -> Result<LocalWorkspaceRow, IpcError> {
        for other in self.list_local_workspaces()? {
            if other.host_alias == w.host_alias
                && other.owner == w.owner
                && other.repo == w.repo
                && other.worktree_key == w.worktree_key
            {
                return Err(IpcError::new(
                    codes::E_CONFLICT,
                    format!(
                        "this worktree is already synced to {}; disconnect it first",
                        other.local_path
                    ),
                ));
            }
            if paths_overlap(&other.local_path, w.local_path) {
                return Err(IpcError::new(
                    codes::E_CONFLICT,
                    format!(
                        "{} overlaps {}, which another worktree already syncs to",
                        w.local_path, other.local_path
                    ),
                ));
            }
        }
        let excludes = serde_json::to_string(w.excludes).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO local_workspaces (host_alias, owner, repo, worktree_key, remote_path,
                                           local_path, session_id, excludes, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'syncing', ?9)",
            rusqlite::params![
                w.host_alias,
                w.owner,
                w.repo,
                w.worktree_key,
                w.remote_path,
                w.local_path,
                w.session_id,
                excludes,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.local_workspace_changed(id);
        self.local_workspace(id)?.ok_or_else(|| not_found(id))
    }

    /// Pause or resume a link. Pausing sets the state to `paused`; resuming
    /// to `syncing` until the next pass says otherwise.
    pub fn set_local_workspace_paused(
        &self,
        id: i64,
        paused: bool,
    ) -> Result<LocalWorkspaceRow, IpcError> {
        let n = self.conn.execute(
            "UPDATE local_workspaces SET paused = ?2, state = ?3 WHERE id = ?1",
            rusqlite::params![id, paused as i64, if paused { "paused" } else { "syncing" }],
        )?;
        if n == 0 {
            return Err(not_found(id));
        }
        self.local_workspace_changed(id);
        self.local_workspace(id)?.ok_or_else(|| not_found(id))
    }

    pub fn set_local_workspace_excludes(
        &self,
        id: i64,
        excludes: &[String],
    ) -> Result<LocalWorkspaceRow, IpcError> {
        let json = serde_json::to_string(excludes).unwrap_or_else(|_| "[]".into());
        let n = self.conn.execute(
            "UPDATE local_workspaces SET excludes = ?2 WHERE id = ?1",
            rusqlite::params![id, json],
        )?;
        if n == 0 {
            return Err(not_found(id));
        }
        self.local_workspace_changed(id);
        self.local_workspace(id)?.ok_or_else(|| not_found(id))
    }

    /// Write a pass's status. A no-op for a link that was removed meanwhile.
    pub fn set_local_workspace_status(
        &self,
        id: i64,
        st: &LocalWorkspaceStatus<'_>,
    ) -> Result<(), IpcError> {
        let n = self.conn.execute(
            "UPDATE local_workspaces
                SET state = ?2, last_error = ?3, pending_local = ?4, pending_remote = ?5,
                    skipped = ?6, last_sync_at = COALESCE(?7, last_sync_at)
              WHERE id = ?1",
            rusqlite::params![
                id,
                st.state,
                st.last_error,
                st.pending_local,
                st.pending_remote,
                st.skipped,
                st.synced_at
            ],
        )?;
        if n > 0 {
            self.local_workspace_changed(id);
        }
        Ok(())
    }

    /// Record the person's pick for one conflict (`local` / `remote`), for
    /// the next pass to carry out. `E_NOTFOUND` when there is no such open
    /// conflict.
    pub fn set_local_conflict_resolution(
        &self,
        id: i64,
        path: &str,
        keep: &str,
    ) -> Result<(), IpcError> {
        if keep != "local" && keep != "remote" {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("keep must be local or remote, not {keep:?}"),
            ));
        }
        let n = self.conn.execute(
            "UPDATE local_workspace_conflicts SET resolution = ?3
              WHERE workspace_id = ?1 AND path = ?2",
            rusqlite::params![id, path, keep],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no open conflict on {path}"),
            ));
        }
        self.local_workspace_changed(id);
        Ok(())
    }

    /// Drop a link with its BASE and conflicts. Files on either side are
    /// never touched. `false` when there was no such link.
    pub fn delete_local_workspace(&self, id: i64) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM local_workspace_files WHERE workspace_id = ?1",
            [id],
        )?;
        tx.execute(
            "DELETE FROM local_workspace_conflicts WHERE workspace_id = ?1",
            [id],
        )?;
        tx.execute(
            "DELETE FROM local_workspace_activity WHERE workspace_id = ?1",
            [id],
        )?;
        let n = tx.execute("DELETE FROM local_workspaces WHERE id = ?1", [id])?;
        tx.commit()?;
        if n > 0 {
            self.local_workspace_changed(id);
        }
        Ok(n > 0)
    }

    /// The link's BASE, by path.
    pub fn local_workspace_base(&self, id: i64) -> Result<HashMap<String, BaseEntry>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, sha256, local_size, local_mtime, remote_size, remote_mtime
               FROM local_workspace_files WHERE workspace_id = ?1",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                BaseEntry {
                    sha256: r.get(1)?,
                    local: stat_of(r.get(2)?, r.get(3)?),
                    remote: stat_of(r.get(4)?, r.get(5)?),
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// Record what a pass did, in one transaction. A no-op for a link that
    /// was removed meanwhile (its rows would only be orphans).
    pub fn apply_local_pass(&self, id: i64, w: &LocalPassWrite, now: i64) -> Result<(), IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = tx
            .query_row("SELECT 1 FROM local_workspaces WHERE id = ?1", [id], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !exists {
            return Ok(());
        }
        {
            let mut up = tx.prepare_cached(
                "INSERT INTO local_workspace_files
                     (workspace_id, path, sha256, local_size, local_mtime, remote_size, remote_mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(workspace_id, path) DO UPDATE SET
                     sha256 = excluded.sha256,
                     local_size = excluded.local_size, local_mtime = excluded.local_mtime,
                     remote_size = excluded.remote_size, remote_mtime = excluded.remote_mtime",
            )?;
            for (path, e) in &w.upsert {
                up.execute(rusqlite::params![
                    id,
                    path,
                    e.sha256,
                    e.local.map(|s| s.size),
                    e.local.map(|s| s.mtime),
                    e.remote.map(|s| s.size),
                    e.remote.map(|s| s.mtime),
                ])?;
            }
            let mut rm = tx.prepare_cached(
                "DELETE FROM local_workspace_files WHERE workspace_id = ?1 AND path = ?2",
            )?;
            for path in &w.remove {
                rm.execute(rusqlite::params![id, path])?;
            }
            let mut add = tx.prepare_cached(
                "INSERT INTO local_workspace_conflicts
                     (workspace_id, path, kind, detected_at, local_sha, local_size, local_mtime,
                      remote_sha, remote_size, remote_mtime, resolution)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)
                 ON CONFLICT(workspace_id, path) DO UPDATE SET
                     kind = excluded.kind,
                     local_sha = excluded.local_sha, local_size = excluded.local_size,
                     local_mtime = excluded.local_mtime, remote_sha = excluded.remote_sha,
                     remote_size = excluded.remote_size, remote_mtime = excluded.remote_mtime,
                     resolution = NULL",
            )?;
            for c in &w.add_conflicts {
                let (l, r) = (c.local.as_ref(), c.remote.as_ref());
                add.execute(rusqlite::params![
                    id,
                    c.path,
                    c.kind,
                    now,
                    l.map(|x| &x.sha256),
                    l.map(|x| x.stat.size),
                    l.map(|x| x.stat.mtime),
                    r.map(|x| &x.sha256),
                    r.map(|x| x.stat.size),
                    r.map(|x| x.stat.mtime),
                ])?;
            }
            let mut clear = tx.prepare_cached(
                "DELETE FROM local_workspace_conflicts WHERE workspace_id = ?1 AND path = ?2",
            )?;
            for path in &w.clear_conflicts {
                clear.execute(rusqlite::params![id, path])?;
            }
            let mut act = tx.prepare_cached(
                "INSERT INTO local_workspace_activity (workspace_id, path, origin, change, at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(workspace_id, path) DO UPDATE SET
                     origin = excluded.origin, change = excluded.change, at = excluded.at",
            )?;
            for (path, origin, change) in &w.activity {
                act.execute(rusqlite::params![id, path, origin, change, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The link's activity log, newest first.
    pub fn local_workspace_activity(&self, id: i64) -> Result<Vec<LocalActivityRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, origin, change, at FROM local_workspace_activity
              WHERE workspace_id = ?1 ORDER BY at DESC, path",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok(LocalActivityRow {
                path: r.get(0)?,
                origin: r.get(1)?,
                change: r.get(2)?,
                at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Forget activity rows: those of `origin` (or both sides when `None`),
    /// and only `paths` when given. Answers how many went.
    pub fn clear_local_workspace_activity(
        &self,
        id: i64,
        origin: Option<&str>,
        paths: Option<&[String]>,
    ) -> Result<usize, IpcError> {
        if let Some(o) = origin {
            if o != "local" && o != "remote" {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("origin must be local or remote, not {o:?}"),
                ));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        match paths {
            None => {
                n += tx.execute(
                    "DELETE FROM local_workspace_activity
                      WHERE workspace_id = ?1 AND (?2 IS NULL OR origin = ?2)",
                    rusqlite::params![id, origin],
                )?;
            }
            Some(paths) => {
                let mut stmt = tx.prepare_cached(
                    "DELETE FROM local_workspace_activity
                      WHERE workspace_id = ?1 AND path = ?2 AND (?3 IS NULL OR origin = ?3)",
                )?;
                for p in paths {
                    n += stmt.execute(rusqlite::params![id, p, origin])?;
                }
            }
        }
        tx.commit()?;
        if n > 0 {
            self.local_workspace_changed(id);
        }
        Ok(n)
    }

    /// Set who drives the worktree. `E_INVALID` for an unknown driver.
    pub fn set_local_workspace_driver(
        &self,
        id: i64,
        driver: &str,
        now: i64,
    ) -> Result<LocalWorkspaceRow, IpcError> {
        if !LOCAL_WORKSPACE_DRIVERS.contains(&driver) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("driver must be shared, developer or agent, not {driver:?}"),
            ));
        }
        let n = self.conn.execute(
            "UPDATE local_workspaces SET driver = ?2, driver_since = ?3 WHERE id = ?1",
            rusqlite::params![id, driver, now],
        )?;
        if n == 0 {
            return Err(not_found(id));
        }
        self.local_workspace_changed(id);
        self.local_workspace(id)?.ok_or_else(|| not_found(id))
    }
}

/// Whether one directory is the other or contains it (string-wise, on
/// absolute paths with `/` or `\` separators).
pub(crate) fn paths_overlap(a: &str, b: &str) -> bool {
    fn norm(p: &str) -> String {
        let p = p.replace('\\', "/");
        let p = p.trim_end_matches('/');
        format!("{p}/")
    }
    let (a, b) = (norm(a), norm(b));
    a.starts_with(&b) || b.starts_with(&a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(key: &'a str, local: &'a str, ex: &'a [String]) -> NewLocalWorkspace<'a> {
        NewLocalWorkspace {
            host_alias: "devbox",
            owner: "acme",
            repo: "app",
            worktree_key: key,
            remote_path: "/home/u/projects/github.com/acme/app",
            local_path: local,
            session_id: None,
            excludes: ex,
        }
    }

    #[test]
    fn a_link_round_trips_with_its_excludes_and_conflicts() {
        let s = Store::open_in_memory().unwrap();
        let ex = vec!["*.log".to_string()];
        let row = s
            .insert_local_workspace(&new("main", "/w/app", &ex), 10)
            .unwrap();
        assert_eq!(row.excludes, ex);
        assert_eq!(row.state, "syncing");
        assert!(!row.paused);

        let write = LocalPassWrite {
            upsert: vec![(
                "a.txt".into(),
                BaseEntry {
                    sha256: "aa".into(),
                    local: Some(FileStat { size: 1, mtime: 2 }),
                    remote: None,
                },
            )],
            add_conflicts: vec![NewLocalConflict {
                path: "b.txt".into(),
                kind: "both_modified",
                local: Some(SideSeen {
                    sha256: "l".into(),
                    stat: FileStat { size: 1, mtime: 1 },
                }),
                remote: None,
            }],
            ..Default::default()
        };
        s.apply_local_pass(row.id, &write, 20).unwrap();
        let base = s.local_workspace_base(row.id).unwrap();
        assert_eq!(base["a.txt"].local, Some(FileStat { size: 1, mtime: 2 }));
        assert_eq!(base["a.txt"].remote, None);
        let got = s.local_workspace(row.id).unwrap().unwrap();
        assert_eq!(got.conflicts.len(), 1);
        assert_eq!(got.conflicts[0].kind, "both_modified");
        assert_eq!(got.conflicts[0].local.as_ref().unwrap().sha256, "l");
        assert!(got.conflicts[0].remote.is_none());
        s.set_local_conflict_resolution(row.id, "b.txt", "remote")
            .unwrap();
        let got = s.local_workspace(row.id).unwrap().unwrap();
        assert_eq!(got.conflicts[0].resolution.as_deref(), Some("remote"));
        // Seeing it move again clears the pick.
        s.apply_local_pass(row.id, &write, 21).unwrap();
        let got = s.local_workspace(row.id).unwrap().unwrap();
        assert_eq!(got.conflicts[0].resolution, None);
        assert_eq!(
            s.set_local_conflict_resolution(row.id, "nope", "local")
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );

        let clear = LocalPassWrite {
            remove: vec!["a.txt".into()],
            clear_conflicts: vec!["b.txt".into()],
            ..Default::default()
        };
        s.apply_local_pass(row.id, &clear, 30).unwrap();
        assert!(s.local_workspace_base(row.id).unwrap().is_empty());
        assert!(s
            .local_workspace(row.id)
            .unwrap()
            .unwrap()
            .conflicts
            .is_empty());

        assert!(s.delete_local_workspace(row.id).unwrap());
        assert!(s.local_workspace(row.id).unwrap().is_none());
        assert!(!s.delete_local_workspace(row.id).unwrap());
    }

    #[test]
    fn one_link_per_worktree_and_no_nested_directories() {
        let s = Store::open_in_memory().unwrap();
        s.insert_local_workspace(&new("main", "/w/app", &[]), 1)
            .unwrap();
        let again = s.insert_local_workspace(&new("main", "/w/other", &[]), 1);
        assert_eq!(again.unwrap_err().code, codes::E_CONFLICT);
        let inside = s.insert_local_workspace(&new("feat", "/w/app/sub", &[]), 1);
        assert_eq!(inside.unwrap_err().code, codes::E_CONFLICT);
        let around = s.insert_local_workspace(&new("feat", "/w", &[]), 1);
        assert_eq!(around.unwrap_err().code, codes::E_CONFLICT);
        // A sibling whose name merely starts the same is fine.
        s.insert_local_workspace(&new("feat", "/w/app-feat", &[]), 1)
            .unwrap();
    }

    #[test]
    fn pause_and_status_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let row = s
            .insert_local_workspace(&new("main", "/w/app", &[]), 1)
            .unwrap();
        let p = s.set_local_workspace_paused(row.id, true).unwrap();
        assert!(p.paused);
        assert_eq!(p.state, "paused");
        s.set_local_workspace_status(
            row.id,
            &LocalWorkspaceStatus {
                state: "conflict",
                pending_local: 2,
                synced_at: Some(99),
                ..Default::default()
            },
        )
        .unwrap();
        let got = s.local_workspace(row.id).unwrap().unwrap();
        assert_eq!((got.state.as_str(), got.pending_local), ("conflict", 2));
        assert_eq!(got.last_sync_at, Some(99));
        assert_eq!(
            s.set_local_workspace_paused(999, true).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn a_pass_for_a_removed_link_writes_nothing() {
        let s = Store::open_in_memory().unwrap();
        let row = s
            .insert_local_workspace(&new("main", "/w/app", &[]), 1)
            .unwrap();
        s.delete_local_workspace(row.id).unwrap();
        let write = LocalPassWrite {
            add_conflicts: vec![NewLocalConflict {
                path: "x".into(),
                kind: "both_added",
                local: None,
                remote: None,
            }],
            ..Default::default()
        };
        s.apply_local_pass(row.id, &write, 1).unwrap();
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM local_workspace_conflicts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
    }
}
