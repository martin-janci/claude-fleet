// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.

// Store is a coherent data-access API; several methods (e.g. `with_transaction`,
// `get_account_by_uuid`, `delete_session`) are currently exercised only by
// `#[cfg(test)]` code, so they read as dead in a non-test build.
#![allow(dead_code)]

use crate::events::{EventBus, NoopEventBus, RowChange};
use rusqlite::{Connection, OptionalExtension, Result};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRow {
    pub id: i64,
    pub owner: String,
    pub repo: String,
    pub base_path: String,
    pub last_session_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub status: String,
    pub notes: Option<String>,
    pub account_uuid: Option<String>,
    pub kind: String,
    pub reviews_session_id: Option<i64>,
    pub worktree_key: Option<String>,
    pub lost_at: Option<i64>,
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
    pub context_pct: Option<f64>,
    pub stuck_kind: Option<String>,
    /// Display label set by the in-session agent via the `set_friendly_name`
    /// MCP tool (migration 016). The sidebar shows this when the user's
    /// "friendly names" toggle is on; falls back to `tmux_name` when NULL.
    pub friendly_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostRow {
    pub alias: String,
    pub ssh_alias: Option<String>,
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub hidden: bool,
    pub last_pinged_at: Option<i64>,
    pub account_uuid: Option<String>,
    pub provisioned: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountRow {
    pub uuid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub seat_tier: Option<String>,
    pub last_seen_at: Option<i64>,
}

/// One row of the append-only per-session event timeline (migration 013).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: i64,
    pub at: i64,
    pub kind: String,
    pub detail: Option<String>,
}

/// One inter-session message (migration 015). The store is the source of
/// truth; pane delivery, if requested, happens separately and best-effort.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMessage {
    pub id: i64,
    pub from_session_id: i64,
    pub to_session_id: i64,
    pub body: String,
    pub kind: String,
    pub sent_at: i64,
    /// Unix-epoch second the recipient first listed this message, or `None`
    /// when still unread.
    pub read_at: Option<i64>,
}

/// One live session to upsert during a reconcile write-burst. `project_id`,
/// `account_uuid`, and `worktree_key` are PRE-RESOLVED by the caller (they
/// require reads — `find_project_id_for_path` / `get_session_account` /
/// `worktree_key_for_path` — that must run before the transaction opens).
pub struct ReconcileSession<'a> {
    pub tmux_name: &'a str,
    pub project_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub account_uuid: Option<String>,
    pub worktree_key: Option<String>,
    // NEW — from claude agents --json:
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
    pub context_pct: Option<f64>,
    pub stuck_kind: Option<String>,
    /// Whether this pass actually captured & analyzed the session's pane. When
    /// `true`, `stuck_kind` is authoritative and a `None` CLEARS any prior stuck
    /// flag; when `false` (capture failed / pane absent) the prior `stuck_kind`
    /// is preserved. Without this, a once-set stuck flag could never clear.
    pub intel_observed: bool,
}

/// All inputs for applying one host's probe result atomically. Consumed by
/// `Store::apply_host_reconcile`.
pub struct HostReconcile<'a> {
    pub alias: &'a str,
    /// Whether the probe succeeded. `false` ⇒ only the host row's
    /// reachability/versions are updated; sessions are left untouched.
    pub reachable: bool,
    pub claude_version: Option<&'a str>,
    pub tmux_version: Option<&'a str>,
    pub last_pinged_at: i64,
    /// Live sessions to upsert (empty / ignored when `!reachable`).
    pub sessions: &'a [ReconcileSession<'a>],
    /// tmux_names to keep; rows on this host not in the set are deleted
    /// (only used when `reachable`).
    pub keep: &'a [String],
}

pub struct Store {
    conn: Connection,
    bus: Arc<dyn EventBus>,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_with_bus(path, Arc::new(NoopEventBus))
    }

    pub fn open_with_bus(path: &std::path::Path, bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn, bus };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            bus: Arc::new(NoopEventBus),
        };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_with_bus_in_memory(bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn, bus };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        // Newer migrations are applied only if not yet recorded. We can't
        // wrap them in CREATE-OR-IGNORE because they ALTER existing tables.
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        // Each ALTER-based migration runs in its OWN transaction, together
        // with the `schema_version` row it inserts. SQLite DDL is
        // transactional, so an interrupted migration rolls back entirely —
        // it can never leave a column half-added, which on the next launch
        // would re-run the migration and fail with "duplicate column",
        // bricking startup.
        if v < 2 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/002_hosts_ssh.sql"))?;
            tx.commit()?;
        }
        if v < 3 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/003_accounts.sql"))?;
            tx.commit()?;
        }
        if v < 4 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/004_session_account.sql"))?;
            tx.commit()?;
        }
        if v < 5 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/005_session_reviews.sql"))?;
            tx.commit()?;
        }
        if v < 6 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/006_session_worktree_key.sql"))?;
            tx.commit()?;
        }
        if v < 7 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/007_indexes.sql"))?;
            tx.commit()?;
        }
        if v < 8 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/008_ghost_sessions.sql"))?;
            tx.commit()?;
        }
        if v < 9 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/009_session_claude_id.sql"))?;
            tx.commit()?;
        }
        if v < 10 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/010_claude_agent_fields.sql"))?;
            tx.commit()?;
        }
        if v < 11 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/011_host_provisioned.sql"))?;
            tx.commit()?;
        }
        if v < 12 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!(
                "../migrations/012_session_context_pressure.sql"
            ))?;
            tx.commit()?;
        }
        if v < 13 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/013_session_events.sql"))?;
            tx.commit()?;
        }
        if v < 14 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/014_last_reconciled_at.sql"))?;
            tx.commit()?;
        }
        if v < 15 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/015_session_messages.sql"))?;
            tx.commit()?;
        }
        if v < 16 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/016_session_friendly_name.sql"))?;
            tx.commit()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn has_table(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
    }

    /// Append one row to the per-session event timeline (migration 013). The
    /// timeline is append-only; callers must treat a write failure as
    /// non-fatal (log + continue) so it can never block the mutation that
    /// produced the event.
    pub fn insert_session_event(
        &self,
        session_id: i64,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_events (session_id, at, kind, detail) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, at, kind, detail],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Return the newest-first event timeline for a session, capped at `limit`.
    /// Ordering is `at DESC, id DESC` so events inserted within the same second
    /// still come back in insertion order (newest first).
    pub fn list_session_events(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, session_id, at, kind, detail FROM session_events \
                 WHERE session_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, limit], |row| {
                Ok(SessionEvent {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    at: row.get(2)?,
                    kind: row.get(3)?,
                    detail: row.get(4)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Insert one inter-session message (migration 015). `sent_at` is stamped
    /// here as the current unix epoch. Returns the new row id so the caller
    /// can include it in the pane-delivery header.
    pub fn insert_message(
        &self,
        from_session_id: i64,
        to_session_id: i64,
        body: &str,
        kind: &str,
    ) -> Result<i64, crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_messages \
                   (from_session_id, to_session_id, body, kind, sent_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![from_session_id, to_session_id, body, kind, at],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Newest-first messages addressed to `to_session_id`, capped at `limit`.
    /// When `unread_only`, only rows whose `read_at IS NULL` are returned.
    pub fn list_inbox(
        &self,
        to_session_id: i64,
        unread_only: bool,
        limit: i64,
    ) -> Result<Vec<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = if unread_only {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at \
             FROM session_messages \
             WHERE to_session_id = ?1 AND read_at IS NULL \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        } else {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at \
             FROM session_messages \
             WHERE to_session_id = ?1 \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![to_session_id, limit], |row| {
                Ok(SessionMessage {
                    id: row.get(0)?,
                    from_session_id: row.get(1)?,
                    to_session_id: row.get(2)?,
                    body: row.get(3)?,
                    kind: row.get(4)?,
                    sent_at: row.get(5)?,
                    read_at: row.get(6)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Mark a set of inbox messages as read. Only rows whose `to_session_id`
    /// matches `recipient` are updated — never mark someone else's mail.
    /// Returns the number of rows that flipped from unread to read.
    pub fn mark_messages_read(
        &self,
        ids: &[i64],
        recipient: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE session_messages SET read_at = ?1 \
             WHERE to_session_id = ?2 AND read_at IS NULL AND id IN ({placeholders})",
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&at, &recipient];
        for id in ids {
            params.push(id);
        }
        let n = self
            .conn
            .execute(sql.as_str(), rusqlite::params_from_iter(params))
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(n)
    }

    /// Read a value from the key/value `settings` table. `None` if absent.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .optional()
    }

    /// Insert or replace a value in the `settings` table.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Record which session is the fleet controller (the calling session that
    /// must not kill/recreate/restart itself without `force`). Stored as two
    /// keys in the `settings` table.
    pub fn set_controller(&self, host: &str, tmux_name: &str) -> Result<()> {
        self.set_setting("controller.host", host)?;
        self.set_setting("controller.tmux", tmux_name)?;
        Ok(())
    }

    /// Read the registered controller as `(host, tmux_name)`. `None` unless
    /// both keys are present.
    pub fn get_controller(&self) -> Result<Option<(String, String)>> {
        let host = self.get_setting("controller.host")?;
        let tmux = self.get_setting("controller.tmux")?;
        Ok(host.zip(tmux))
    }

    // ---- Private fetch helpers used after writes to produce emit payloads ----
    //
    // The row-mapping SQL lives in free `fetch_*` functions that take a bare
    // `&Connection` so it can be reused both by these `&self` helpers AND by
    // the `_in_tx` mutation variants (a `&Transaction` derefs to `&Connection`).

    pub fn get_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session(&self.conn, tmux_name, host_alias)
    }

    fn get_host(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
    }

    fn get_project(&self, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
        fetch_project(&self.conn, id)
    }

    fn get_worktree(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch FROM worktrees WHERE id=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
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
    /// Within each project worktrees are ordered by id.
    pub fn list_projects_joined(
        &self,
    ) -> Result<Vec<crate::service::projects::ProjectTreeRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.owner, p.repo, p.base_path, p.last_session_at,
                    w.id, w.project_id, w.name, w.path, w.branch
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id
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
                    name: wname,
                    path: wpath,
                    branch: wbranch,
                });
            }
        }
        Ok(out)
    }

    pub fn upsert_worktree(
        &self,
        project_id: i64,
        name: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktrees (project_id, name, path, branch) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, name) DO UPDATE SET path=excluded.path, branch=excluded.branch",
            rusqlite::params![project_id, name, path, branch],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM worktrees WHERE project_id=?1 AND name=?2",
            rusqlite::params![project_id, name],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_worktree(id)? {
            self.bus.worktree_updated(&row);
        }
        Ok(id)
    }

    pub fn list_worktrees_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch FROM worktrees WHERE project_id=?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_worktrees_not_in(
        &self,
        project_id: i64,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return self.conn.execute(
                "DELETE FROM worktrees WHERE project_id=?1",
                rusqlite::params![project_id],
            );
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql =
            format!("DELETE FROM worktrees WHERE project_id=?1 AND name NOT IN ({placeholders})");
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&project_id];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
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
    pub fn delete_project(&self, project_id: i64) -> Result<(), crate::ipc_error::IpcError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(crate::ipc_error::IpcError::from)?;
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
        Ok(())
    }

    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn upsert_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // Check existence first so `host_added` fires only on a genuine
        // insert. Reconcile calls this for `local` every run; without the
        // check it emitted a spurious host:added (plus a get_host fetch)
        // on every window focus.
        let existed = self
            .conn
            .query_row("SELECT 1 FROM hosts WHERE alias=?1", [alias], |_| Ok(()))
            .optional()?
            .is_some();
        self.conn.execute(
            "INSERT INTO hosts (alias, reachable) VALUES (?1, 1)
             ON CONFLICT(alias) DO UPDATE SET reachable=1",
            rusqlite::params![alias],
        )?;
        if !existed {
            if let Some(row) = self.get_host(alias)? {
                self.bus.host_added(&row);
            }
        }
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                    last_pinged_at, account_uuid, provisioned
             FROM hosts
             ORDER BY (alias='local') DESC, alias ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostRow {
                alias: row.get(0)?,
                ssh_alias: row.get(1)?,
                reachable: row.get::<_, i64>(2)? != 0,
                claude_version: row.get(3)?,
                tmux_version: row.get(4)?,
                hidden: row.get::<_, i64>(5)? != 0,
                last_pinged_at: row.get(6)?,
                account_uuid: row.get(7)?,
                provisioned: row.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn insert_host(&self, alias: &str, ssh_alias: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_added(&row);
        }
        Ok(())
    }

    pub fn update_host_probe(
        &self,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn set_host_hidden(&self, alias: &str, hidden: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET hidden=?1 WHERE alias=?2",
            rusqlite::params![if hidden { 1 } else { 0 }, alias],
        )?;
        // Emit like every other host mutation — the HostRow carries `hidden`,
        // so subscribers see the toggle without a manual refetch.
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts
             ORDER BY uuid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_account(&self, a: &AccountRow) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO accounts (uuid, email, display_name, organization_name,
                                   organization_uuid, seat_tier, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(uuid) DO UPDATE SET
               email=excluded.email,
               display_name=excluded.display_name,
               organization_name=excluded.organization_name,
               organization_uuid=excluded.organization_uuid,
               seat_tier=excluded.seat_tier,
               last_seen_at=excluded.last_seen_at",
            rusqlite::params![
                a.uuid,
                a.email,
                a.display_name,
                a.organization_name,
                a.organization_uuid,
                a.seat_tier,
                a.last_seen_at
            ],
        )?;
        self.bus.account_upserted(a);
        Ok(())
    }

    pub fn get_account_by_uuid(&self, uuid: &str) -> Result<Option<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT uuid, email, display_name, organization_name, organization_uuid,
                    seat_tier, last_seen_at
             FROM accounts WHERE uuid=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![uuid], |row| {
            Ok(AccountRow {
                uuid: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get(2)?,
                organization_name: row.get(3)?,
                organization_uuid: row.get(4)?,
                seat_tier: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn set_host_account(
        &self,
        alias: &str,
        account_uuid: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET account_uuid=?1 WHERE alias=?2",
            rusqlite::params![account_uuid, alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_probed(&row);
        }
        Ok(())
    }

    pub fn set_host_provisioned(
        &self,
        alias: &str,
        provisioned: bool,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET provisioned=?1 WHERE alias=?2",
            rusqlite::params![if provisioned { 1 } else { 0 }, alias],
        )?;
        Ok(())
    }

    pub fn delete_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // The `local` host is never removed.
        if alias == "local" {
            return Ok(());
        }
        // Collect orphaned session ids first so we can emit a `session_killed`
        // event per row — otherwise frontend stores subscribed to session events
        // would carry stale rows that point to a host that no longer exists.
        let orphan_ids: Vec<i64> = self
            .conn
            .prepare_cached("SELECT id FROM sessions WHERE host_alias=?1")?
            .query_map(rusqlite::params![alias], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        self.conn.execute(
            "DELETE FROM sessions WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        self.conn
            .execute("DELETE FROM hosts WHERE alias=?1", rusqlite::params![alias])?;
        for id in &orphan_ids {
            self.bus.session_killed(*id);
        }
        self.bus.host_removed(alias);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        status: &str,
        account_uuid: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        // Check existence before the write so we can distinguish created vs updated.
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        // INSERT ... RETURNING id — one statement instead of the old
        // INSERT then separate `SELECT id`.
        let id: i64 = self.conn.query_row(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid
             RETURNING id",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                status,
                account_uuid
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    /// Upsert a synthetic `kind='bg'` session for a `claude --bg` agent that has
    /// NO matching tmux session. The sentinel `tmux_name` (`bg:<sessionId>`)
    /// keeps it unique under the `(host_alias, tmux_name)` constraint and signals
    /// to the UI that there is no tmux pane to attach. Refreshes the live
    /// `claude_status` on every reconcile; the row's `kind='bg'` exempts it from
    /// ghost cleanup (it is never in the tmux `keep` set).
    pub fn upsert_bg_session(
        &self,
        host_alias: &str,
        tmux_name: &str,
        project_id: Option<i64>,
        claude_session_id: &str,
        claude_status: Option<&str>,
        last_activity_at: i64,
    ) -> Result<i64, rusqlite::Error> {
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        let id: i64 = self.conn.query_row(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, kind,
                                   claude_session_id, claude_status)
             VALUES (?1, ?2, ?3, NULL, ?4, ?4, 'running', 'bg', ?5, ?6)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=COALESCE(excluded.project_id, project_id),
               last_activity_at=excluded.last_activity_at,
               kind='bg',
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               claude_status=COALESCE(excluded.claude_status, claude_status)
             RETURNING id",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                last_activity_at,
                claude_session_id,
                claude_status
            ],
            |row| row.get(0),
        )?;
        if let Some(row) = self.get_session(tmux_name, host_alias)? {
            if existing_id.is_none() {
                self.bus.session_created(&row);
            } else {
                self.bus.session_updated(&row);
            }
        }
        Ok(id)
    }

    pub fn get_session_account(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![host_alias, tmux_name], |row| {
            row.get::<_, Option<String>>(0)
        })?;
        match rows.next() {
            Some(r) => Ok(r?),
            None => Ok(None),
        }
    }

    pub fn list_sessions_for_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key, lost_at,
                    claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
             FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                tmux_name: row.get(1)?,
                host_alias: row.get(2)?,
                project_id: row.get(3)?,
                worktree_id: row.get(4)?,
                created_at: row.get(5)?,
                last_activity_at: row.get(6)?,
                status: row.get(7)?,
                notes: row.get(8)?,
                account_uuid: row.get(9)?,
                kind: row.get(10)?,
                reviews_session_id: row.get(11)?,
                worktree_key: row.get(12)?,
                lost_at: row.get(13)?,
                claude_session_id: row.get(14)?,
                claude_status: row.get(15)?,
                effort_level: row.get(16)?,
                pr_url: row.get(17)?,
                current_activity: row.get(18)?,
                context_pct: row.get(19)?,
                stuck_kind: row.get(20)?,
                friendly_name: row.get(21)?,
            })
        })?;
        rows.collect()
    }

    /// All sessions across every host, in one query. Used by `reconcile_sessions`
    /// to collect its return value once at the end instead of N per-host reads.
    pub fn list_all_sessions(&self) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key, lost_at,
                    claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
             FROM sessions ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                tmux_name: row.get(1)?,
                host_alias: row.get(2)?,
                project_id: row.get(3)?,
                worktree_id: row.get(4)?,
                created_at: row.get(5)?,
                last_activity_at: row.get(6)?,
                status: row.get(7)?,
                notes: row.get(8)?,
                account_uuid: row.get(9)?,
                kind: row.get(10)?,
                reviews_session_id: row.get(11)?,
                worktree_key: row.get(12)?,
                lost_at: row.get(13)?,
                claude_session_id: row.get(14)?,
                claude_status: row.get(15)?,
                effort_level: row.get(16)?,
                pr_url: row.get(17)?,
                current_activity: row.get(18)?,
                context_pct: row.get(19)?,
                stuck_kind: row.get(20)?,
                friendly_name: row.get(21)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_related_sessions(
        &self,
        session_id: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // Look up source's (project_id, worktree_key) first.
        let (proj, key): (Option<i64>, Option<String>) = self.conn.query_row(
            "SELECT project_id, worktree_key FROM sessions WHERE id=?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Orphans (project_id=NULL) have no relateds — they share no identity.
        let Some(project_id) = proj else {
            return Ok(Vec::new());
        };
        // A project-having session always has a worktree_key after reconcile
        // ("main" at minimum). A NULL key (legacy/pre-reconcile) matches nothing.
        let Some(key) = key else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key, lost_at,
                    claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
             FROM sessions
             WHERE project_id=?1 AND worktree_key=?2 AND id<>?3
             ORDER BY host_alias ASC, tmux_name ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id, key, session_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                tmux_name: row.get(1)?,
                host_alias: row.get(2)?,
                project_id: row.get(3)?,
                worktree_id: row.get(4)?,
                created_at: row.get(5)?,
                last_activity_at: row.get(6)?,
                status: row.get(7)?,
                notes: row.get(8)?,
                account_uuid: row.get(9)?,
                kind: row.get(10)?,
                reviews_session_id: row.get(11)?,
                worktree_key: row.get(12)?,
                lost_at: row.get(13)?,
                claude_session_id: row.get(14)?,
                claude_status: row.get(15)?,
                effort_level: row.get(16)?,
                pr_url: row.get(17)?,
                current_activity: row.get(18)?,
                context_pct: row.get(19)?,
                stuck_kind: row.get(20)?,
                friendly_name: row.get(21)?,
            })
        })?;
        rows.collect()
    }

    /// Mark a session as a review of `reviews_session_id` (or back to 'work' with
    /// None). Write-once at spawn_review time. Reconcile never touches these
    /// columns — they survive re-probe because upsert_session's ON CONFLICT clause
    /// omits them.
    pub fn set_session_kind(
        &self,
        id: i64,
        kind: &str,
        reviews_session_id: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET kind = ?1, reviews_session_id = ?2 WHERE id = ?3",
            rusqlite::params![kind, reviews_session_id, id],
        )?;
        if let Some(row) = self.get_session_by_id(id)? {
            self.bus.session_updated(&row);
        }
        Ok(())
    }

    /// Record the Claude Code session id minted for a session. Reconcile's
    /// `upsert_session` never writes this column, so the value survives
    /// reconciliation.
    pub fn set_claude_session_id(&self, id: i64, uuid: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET claude_session_id=?1 WHERE id=?2",
            rusqlite::params![uuid, id],
        )?;
        Ok(())
    }

    /// Set a session's portable worktree key (derived from its cwd by reconcile).
    /// Emits `session_updated` so the frontend patches in place.
    pub fn set_worktree_key(&self, id: i64, key: Option<&str>) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET worktree_key = ?1 WHERE id = ?2",
            rusqlite::params![key, id],
        )?;
        if let Some(row) = self.get_session_by_id(id)? {
            self.bus.session_updated(&row);
        }
        Ok(())
    }

    /// Set the session's display label (migration 016). `None` clears it.
    /// Emits `session_updated` so the sidebar patches in place.
    pub fn set_friendly_name(
        &self,
        host_alias: &str,
        tmux_name: &str,
        friendly_name: Option<&str>,
    ) -> Result<Option<SessionRow>, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE sessions SET friendly_name = ?1 \
             WHERE host_alias = ?2 AND tmux_name = ?3",
            rusqlite::params![friendly_name, host_alias, tmux_name],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let row = fetch_session(&self.conn, tmux_name, host_alias)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    /// Transition a session back to running (clears `lost_at`). Called by the
    /// `recreate_session` flow after `new_session` rebuilds the tmux session on
    /// the host — for both ghost and live (RAM/wedged) recreates.
    pub fn restore_session(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET status='running', lost_at=NULL WHERE id=?1",
            rusqlite::params![id],
        )?;
        let row = fetch_session_by_id(&self.conn, id)?;
        if let Some(ref r) = row {
            self.bus.session_updated(r);
        }
        Ok(row)
    }

    pub fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        fetch_session_by_id(&self.conn, id)
    }

    pub fn get_host_row(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
        fetch_host(&self.conn, alias)
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

    pub fn project_base_path(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT base_path FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Run `f` under the implicit lock and return its result.
    ///
    /// The helper exists for documentation: at call sites,
    /// `let data = { let s = store.lock().unwrap(); s.with_snapshot(|s| s.list_hosts()) };`
    /// makes it visible that the lock is held only for the duration of the closure
    /// — and downstream readers can see the I/O happens after the lock drops.
    pub fn with_snapshot<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Store) -> R,
    {
        f(self)
    }

    /// Run `f` inside a single `conn.transaction()`. Used by reconcile paths
    /// that batch many upserts/deletes after a fan-out of off-lock probes —
    /// one fsync per batch instead of one per row.
    pub fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R>
    where
        F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }

    // ---- Reconcile write-burst: single transaction + emit-after-commit ----
    //
    // The `*_in_tx` helpers below run ONLY their SQL against an ambient
    // `&Transaction` and return a `RowChange` describing the event to emit —
    // they do NOT touch `self.bus`. `apply_host_reconcile` drives them inside
    // one transaction, commits, and only THEN flushes the collected changes to
    // the bus. A mid-batch error rolls the whole transaction back, so no event
    // fires for a write that didn't persist.
    //
    // The public `update_host_probe` / `upsert_session` /
    // `touch_project_last_session_at` / `delete_sessions_not_in` methods are
    // intentionally left untouched — direct (non-reconcile) callers keep
    // emitting immediately. Note: `ghost_and_clean_sessions_in_tx` has no
    // public twin by design — reconcile is the only caller.
    //
    // MAINTENANCE: each `*_in_tx` helper deliberately mirrors the SQL of its
    // public twin (same column lists, same upsert ON CONFLICT clause, same
    // SELECT-ids-before-DELETE). They differ ONLY in: (a) `tx` vs `self.conn`,
    // and (b) collecting a `RowChange` vs emitting via `self.bus`. If you change
    // a schema/SQL detail in a public method, change its `_in_tx` twin too.
    // Both paths are test-covered (direct: the `*_emits_*` event tests; tx: the
    // `apply_host_reconcile` rollback + happy-path tests), so a divergence will
    // surface as a test failure rather than silent corruption.
    //
    // `worktree_key` is written by `upsert_session_in_tx` ONLY — the public
    // `upsert_session` intentionally omits it (reconcile is the only path that
    // knows the session's cwd and can compute the key).

    fn update_host_probe_in_tx(
        tx: &rusqlite::Transaction,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        if let Some(row) = fetch_host(tx, alias)? {
            out.push(RowChange::HostProbed(row));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_session_in_tx(
        tx: &rusqlite::Transaction,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        account_uuid: Option<&str>,
        worktree_key: Option<&str>,
        claude_session_id: Option<&str>,
        claude_status: Option<&str>,
        effort_level: Option<&str>,
        pr_url: Option<&str>,
        current_activity: Option<&str>,
        context_pct: Option<f64>,
        stuck_kind: Option<&str>,
        intel_observed: bool,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // Check existence before the write so we can distinguish created vs updated.
        let existing_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
                rusqlite::params![tmux_name, host_alias],
                |row| row.get(0),
            )
            .optional()?;

        tx.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid,
                                   worktree_key, lost_at,
                                   claude_session_id, claude_status, effort_level, pr_url, current_activity,
                                   context_pct, stuck_kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               last_activity_at=excluded.last_activity_at,
               account_uuid=COALESCE(excluded.account_uuid, account_uuid),
               worktree_key=COALESCE(excluded.worktree_key, worktree_key),
               status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
               lost_at=NULL,
               claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
               claude_status=COALESCE(excluded.claude_status, claude_status),
               effort_level=COALESCE(excluded.effort_level, effort_level),
               pr_url=COALESCE(excluded.pr_url, pr_url),
               current_activity=COALESCE(excluded.current_activity, current_activity),
               context_pct=COALESCE(excluded.context_pct, context_pct),
               -- stuck_kind is authoritative when the pane was observed this
               -- pass (?16): a NULL then CLEARS a stale flag. When the pane was
               -- NOT observed (capture failed) we preserve the prior value.
               stuck_kind=CASE WHEN ?16 THEN excluded.stuck_kind
                               ELSE COALESCE(excluded.stuck_kind, stuck_kind) END",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                account_uuid,
                worktree_key,
                claude_session_id,
                claude_status,
                effort_level,
                pr_url,
                current_activity,
                context_pct,
                stuck_kind,
                intel_observed
            ],
        )?;
        if let Some(row) = fetch_session(tx, tmux_name, host_alias)? {
            if existing_id.is_none() {
                out.push(RowChange::SessionCreated(row));
            } else {
                out.push(RowChange::SessionUpdated(row));
            }
        }
        Ok(())
    }

    fn touch_project_last_session_at_in_tx(
        tx: &rusqlite::Transaction,
        project_id: i64,
        ts: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        tx.execute(
            "UPDATE projects SET last_session_at = MAX(COALESCE(last_session_at, 0), ?1) WHERE id = ?2",
            rusqlite::params![ts, project_id],
        )?;
        if let Some(row) = fetch_project(tx, project_id)? {
            out.push(RowChange::ProjectUpdated(row));
        }
        Ok(())
    }

    /// Phase 1: sessions not in `keep_names` that are currently live (`status !=
    /// 'ghost'`) are soft-deleted by setting `status='ghost'` and `lost_at=now`.
    /// Phase 2: sessions that are already ghost (from a previous cycle) and still
    /// not in `keep_names` are hard-deleted.
    ///
    /// `kind='bg'` rows are EXCLUDED from both phases: background (`claude --bg`)
    /// sessions are never tmux sessions, so they can never appear in
    /// `keep_names`. Ghosting them on every reconcile would be wrong — they're
    /// surfaced from `claude agents --json`, not from tmux.
    fn ghost_and_clean_sessions_in_tx(
        tx: &rusqlite::Transaction,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        out: &mut Vec<RowChange>,
    ) -> Result<(), rusqlite::Error> {
        // ── Phase 2 prep: collect already-ghost IDs BEFORE Phase 1 modifies rows
        // so that sessions newly ghosted in Phase 1 are not immediately deleted.
        let pre_ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "SELECT id FROM sessions WHERE host_alias=?1 AND status='ghost' AND kind!='bg'",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND kind!='bg' AND tmux_name NOT IN ({phs})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        // ── Phase 1: ghost live sessions not in keep ──────────────────────────
        let ghost_ids: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare_cached(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind!='bg'
                 RETURNING id",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![now, host_alias], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let phs = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1
                 WHERE host_alias=?2 AND status!='ghost' AND kind!='bg' AND tmux_name NOT IN ({phs})
                 RETURNING id"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        for id in &ghost_ids {
            if let Some(row) = fetch_session_by_id(tx, *id)? {
                out.push(RowChange::SessionUpdated(row));
            }
        }

        // ── Phase 2: hard-delete sessions that were already ghost before this cycle
        if !pre_ghost_ids.is_empty() {
            let phs = pre_ghost_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM sessions WHERE id IN ({phs})");
            let params: Vec<&dyn rusqlite::ToSql> = pre_ghost_ids
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect();
            tx.execute(&sql, params.as_slice())?;
            for id in &pre_ghost_ids {
                out.push(RowChange::SessionKilled(*id));
            }
        }

        Ok(())
    }

    /// One probed/live session to apply during a reconcile write-burst, with
    /// its `(project_id, account_uuid)` ALREADY resolved by the caller (those
    /// are reads — `find_project_id_for_path` / `get_session_account` — and
    /// must happen before the transaction opens).
    pub fn apply_host_reconcile(&mut self, spec: HostReconcile<'_>) -> Result<(), rusqlite::Error> {
        // Phase 1: run all SQL inside one transaction, collecting RowChanges.
        let changes = self.with_transaction(|tx| {
            let mut out: Vec<RowChange> = Vec::new();
            Self::update_host_probe_in_tx(
                tx,
                spec.alias,
                spec.reachable,
                spec.claude_version,
                spec.tmux_version,
                spec.last_pinged_at,
                &mut out,
            )?;
            // Only a reachable probe rewrites the session set. An unreachable
            // host keeps its last-known rows (no upserts, no delete-not-in).
            if spec.reachable {
                // Accumulate the latest activity per project, then touch each
                // project ONCE — N sessions in one project would otherwise
                // fire N redundant UPDATEs + N `project:updated` events.
                let mut project_touch: std::collections::HashMap<i64, i64> =
                    std::collections::HashMap::new();
                for sess in spec.sessions {
                    Self::upsert_session_in_tx(
                        tx,
                        sess.tmux_name,
                        spec.alias,
                        sess.project_id,
                        None,
                        sess.created_at,
                        sess.last_activity_at,
                        sess.account_uuid.as_deref(),
                        sess.worktree_key.as_deref(),
                        sess.claude_session_id.as_deref(),
                        sess.claude_status.as_deref(),
                        sess.effort_level.as_deref(),
                        sess.pr_url.as_deref(),
                        sess.current_activity.as_deref(),
                        sess.context_pct,
                        sess.stuck_kind.as_deref(),
                        sess.intel_observed,
                        &mut out,
                    )?;
                    if let Some(pid) = sess.project_id {
                        let latest = project_touch.entry(pid).or_insert(0);
                        *latest = (*latest).max(sess.last_activity_at);
                    }
                }
                for (pid, ts) in project_touch {
                    Self::touch_project_last_session_at_in_tx(tx, pid, ts, &mut out)?;
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                Self::ghost_and_clean_sessions_in_tx(tx, spec.alias, spec.keep, now, &mut out)?;
            }
            Ok(out)
        })?;

        // Phase 2: transaction committed — now it is safe to emit.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
    }

    /// Stamp `last_reconciled_at = at` on the sessions a reconcile pass just
    /// observed live on `host_alias` (the `keep` set). This is the proactive
    /// freshness marker the Wave-2 background tick (Task H) relies on so the
    /// frontend can gray out rows whose host has gone quiet. Best-effort and
    /// emit-free: it does not change any user-visible row field, so it neither
    /// fires row events nor aborts reconcile on failure.
    pub fn mark_sessions_reconciled(
        &self,
        host_alias: &str,
        keep_names: &[String],
        at: i64,
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return Ok(0);
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE sessions SET last_reconciled_at=?1 \
             WHERE host_alias=?2 AND tmux_name IN ({placeholders})"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&at, &host_alias];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
    }

    pub fn delete_session(&self, id: i64) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        self.bus.session_killed(id);
        Ok(())
    }

    pub fn delete_sessions_not_in(
        &self,
        host_alias: &str,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        // `DELETE ... RETURNING id` — delete and collect deleted ids in one
        // statement (no separate SELECT-then-DELETE).
        let ids_to_delete: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = self
                .conn
                .prepare_cached("DELETE FROM sessions WHERE host_alias=?1 RETURNING id")?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders}) RETURNING id"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        for id in &ids_to_delete {
            self.bus.session_killed(*id);
        }
        Ok(ids_to_delete.len())
    }

    /// Update `claude_status` for the session whose `claude_session_id` matches.
    /// No-ops silently when no row matches (hook arrived before reconcile enriched it).
    pub fn set_claude_status_by_session_id(
        &self,
        claude_session_id: &str,
        status: &str,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET claude_status = ?1 WHERE claude_session_id = ?2",
                rusqlite::params![status, claude_session_id],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        if changed > 0 {
            // Emit session_updated so the frontend patches the row in real-time.
            if let Ok(row) = self.fetch_session_by_claude_id(claude_session_id) {
                self.bus.session_updated(&row);
            }
        }
        Ok(())
    }

    fn fetch_session_by_claude_id(
        &self,
        claude_session_id: &str,
    ) -> Result<SessionRow, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                        last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                        worktree_key, lost_at,
                        claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
                 FROM sessions WHERE claude_session_id = ?1",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        stmt.query_row(rusqlite::params![claude_session_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                tmux_name: row.get(1)?,
                host_alias: row.get(2)?,
                project_id: row.get(3)?,
                worktree_id: row.get(4)?,
                created_at: row.get(5)?,
                last_activity_at: row.get(6)?,
                status: row.get(7)?,
                notes: row.get(8)?,
                account_uuid: row.get(9)?,
                kind: row.get(10)?,
                reviews_session_id: row.get(11)?,
                worktree_key: row.get(12)?,
                lost_at: row.get(13)?,
                claude_session_id: row.get(14)?,
                claude_status: row.get(15)?,
                effort_level: row.get(16)?,
                pr_url: row.get(17)?,
                current_activity: row.get(18)?,
                context_pct: row.get(19)?,
                stuck_kind: row.get(20)?,
                friendly_name: row.get(21)?,
            })
        })
        .map_err(crate::ipc_error::IpcError::from)
    }
}

// ---- Connection-level row fetch helpers ----
//
// Free functions (not methods) so they accept a bare `&Connection`. A
// `&Transaction` derefs to `&Connection`, so the same SQL serves both the
// autocommit `&self` helpers and the transactional `_in_tx` mutation paths
// without duplicating the row-mapping closures.

fn fetch_session(
    conn: &Connection,
    tmux_name: &str,
    host_alias: &str,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key, lost_at,
                claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
         FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![tmux_name, host_alias], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            tmux_name: row.get(1)?,
            host_alias: row.get(2)?,
            project_id: row.get(3)?,
            worktree_id: row.get(4)?,
            created_at: row.get(5)?,
            last_activity_at: row.get(6)?,
            status: row.get(7)?,
            notes: row.get(8)?,
            account_uuid: row.get(9)?,
            kind: row.get(10)?,
            reviews_session_id: row.get(11)?,
            worktree_key: row.get(12)?,
            lost_at: row.get(13)?,
            claude_session_id: row.get(14)?,
            claude_status: row.get(15)?,
            effort_level: row.get(16)?,
            pr_url: row.get(17)?,
            current_activity: row.get(18)?,
            context_pct: row.get(19)?,
            stuck_kind: row.get(20)?,
            friendly_name: row.get(21)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_session_by_id(conn: &Connection, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key, lost_at,
                claude_session_id, claude_status, effort_level, pr_url, current_activity,
                    context_pct, stuck_kind, friendly_name
         FROM sessions WHERE id=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            tmux_name: row.get(1)?,
            host_alias: row.get(2)?,
            project_id: row.get(3)?,
            worktree_id: row.get(4)?,
            created_at: row.get(5)?,
            last_activity_at: row.get(6)?,
            status: row.get(7)?,
            notes: row.get(8)?,
            account_uuid: row.get(9)?,
            kind: row.get(10)?,
            reviews_session_id: row.get(11)?,
            worktree_key: row.get(12)?,
            lost_at: row.get(13)?,
            claude_session_id: row.get(14)?,
            claude_status: row.get(15)?,
            effort_level: row.get(16)?,
            pr_url: row.get(17)?,
            current_activity: row.get(18)?,
            context_pct: row.get(19)?,
            stuck_kind: row.get(20)?,
            friendly_name: row.get(21)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_host(conn: &Connection, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                last_pinged_at, account_uuid, provisioned
         FROM hosts WHERE alias=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![alias], |row| {
        Ok(HostRow {
            alias: row.get(0)?,
            ssh_alias: row.get(1)?,
            reachable: row.get::<_, i64>(2)? != 0,
            claude_version: row.get(3)?,
            tmux_version: row.get(4)?,
            hidden: row.get::<_, i64>(5)? != 0,
            last_pinged_at: row.get(6)?,
            account_uuid: row.get(7)?,
            provisioned: row.get::<_, i64>(8)? != 0,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_project(conn: &Connection, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, owner, repo, base_path, last_session_at FROM projects WHERE id=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| {
        Ok(ProjectRow {
            id: row.get(0)?,
            owner: row.get(1)?,
            repo: row.get(2)?,
            base_path: row.get(3)?,
            last_session_at: row.get(4)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TABLES: &[&str] = &[
        "hosts",
        "projects",
        "worktrees",
        "sessions",
        "handoffs",
        "settings",
        "schema_version",
        "session_events",
        "session_messages",
    ];

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in EXPECTED_TABLES {
            assert!(store.has_table(t).expect("has_table"), "missing table: {t}");
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(store.schema_version().expect("version"), 16);
    }

    #[test]
    fn session_events_insert_then_list_newest_first_with_limit() {
        let s = Store::open_in_memory().expect("open");
        // Insert several events for session 7 (and a decoy for another session).
        s.insert_session_event(7, "status_change", Some("working"))
            .unwrap();
        s.insert_session_event(7, "prompt_sent", Some("hello"))
            .unwrap();
        s.insert_session_event(7, "stuck", Some("auth_menu"))
            .unwrap();
        s.insert_session_event(99, "killed", None).unwrap();

        // Newest-first (at DESC, id DESC): same-second inserts come back in
        // reverse insertion order.
        let all = s.list_session_events(7, 50).unwrap();
        assert_eq!(all.len(), 3, "decoy session 99 must be excluded");
        assert_eq!(all[0].kind, "stuck");
        assert_eq!(all[0].detail.as_deref(), Some("auth_menu"));
        assert_eq!(all[1].kind, "prompt_sent");
        assert_eq!(all[2].kind, "status_change");
        assert_eq!(all[2].session_id, 7);

        // Limit caps the result to the newest N.
        let limited = s.list_session_events(7, 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].kind, "stuck");
        assert_eq!(limited[1].kind, "prompt_sent");

        // NULL detail round-trips.
        let other = s.list_session_events(99, 50).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].detail, None);
    }

    #[test]
    fn session_messages_inbox_roundtrip_and_mark_read() {
        let s = Store::open_in_memory().expect("open");
        // Two messages to session 5, one decoy to session 9.
        let m1 = s.insert_message(1, 5, "hello", "message").unwrap();
        let m2 = s.insert_message(2, 5, "second", "task").unwrap();
        s.insert_message(1, 9, "noise", "message").unwrap();

        // list_inbox returns newest-first and excludes the decoy.
        let all = s.list_inbox(5, false, 50).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, m2);
        assert_eq!(all[0].body, "second");
        assert_eq!(all[0].kind, "task");
        assert_eq!(all[1].id, m1);
        assert!(all.iter().all(|m| m.read_at.is_none()));

        // unread_only filter and limit.
        let unread = s.list_inbox(5, true, 1).unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, m2);

        // Mark only one as read; the other stays unread. A mismatched
        // recipient cannot mark someone else's mail.
        let updated = s.mark_messages_read(&[m1, m2], 5).unwrap();
        assert_eq!(updated, 2);
        let again = s.list_inbox(5, true, 50).unwrap();
        assert!(again.is_empty(), "all unread were marked");
        let foreign = s.mark_messages_read(&[m1], 9).unwrap();
        assert_eq!(foreign, 0, "wrong recipient cannot mark");
    }

    #[test]
    fn controller_set_get_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_controller().unwrap(), None, "unset is None");
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        // overwrite
        s.set_controller("mefistos", "ctrl").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mefistos".to_string(), "ctrl".to_string()))
        );
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().expect("open");
        let on: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(on, 1, "foreign_keys pragma should be ON");
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
            .delete_worktrees_not_in(pid, &["main".to_string(), "feature-x".to_string()])
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
    fn upsert_and_list_sessions_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("dev-foo", "local", None, None, 1000, 2000, "running", None)
            .unwrap();
        assert!(id > 0);
        let id2 = s
            .upsert_session("dev-foo", "local", None, None, 1000, 3000, "running", None)
            .unwrap();
        assert_eq!(id, id2);
        let rows = s.list_sessions_for_host("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_activity_at, 3000);
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
    fn sessions_prune_removes_stale_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-b", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-c", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let removed = s
            .delete_sessions_not_in("local", &["dev-a".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(s.list_sessions_for_host("local").unwrap().len(), 1);
    }

    #[test]
    fn migration_002_adds_ssh_alias_column_to_hosts() {
        let s = Store::open_in_memory().expect("open");
        // sqlite_master pragma_table_info path
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "ssh_alias"),
            "expected `ssh_alias` column; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_seven_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 16);
    }

    #[test]
    fn deleting_a_reviewed_source_nulls_the_review_link_not_errors() {
        // Self-FK uses ON DELETE SET NULL: deleting a source session that a
        // review still points at must succeed (link nulls), not fail the FK.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        // Delete the source while the review still references it.
        store
            .delete_session(src)
            .expect("delete source must not trip the self-FK");
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(
            row.reviews_session_id, None,
            "link should be nulled by ON DELETE SET NULL"
        );
        assert_eq!(row.kind, "review", "the review row itself survives");
    }

    #[test]
    fn migration_004_adds_account_uuid_column_to_sessions() {
        let s = Store::open_in_memory().expect("open");
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('sessions')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on sessions; got: {cols:?}"
        );
    }

    #[test]
    fn migration_003_adds_accounts_table_and_host_account_uuid_column() {
        let s = Store::open_in_memory().expect("open");
        assert!(
            s.has_table("accounts").expect("has_table"),
            "expected accounts table"
        );
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on hosts; got: {cols:?}"
        );
    }

    #[test]
    fn list_hosts_orders_local_first_then_alpha() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.insert_host("zebra", Some("zebra")).unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let names: Vec<String> = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(names, vec!["local", "mefistos", "zebra"]);
    }

    #[test]
    fn insert_host_records_ssh_alias() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mefistos")
            .unwrap();
        assert_eq!(row.ssh_alias.as_deref(), Some("mefistos"));
        assert!(!row.reachable);
        assert!(!row.hidden);
    }

    #[test]
    fn update_host_probe_persists_versions_and_reachability() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.update_host_probe("h", true, Some("2.1.144"), Some("3.6a"), 1000)
            .unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|x| x.alias == "h")
            .unwrap();
        assert!(row.reachable);
        assert_eq!(row.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(row.tmux_version.as_deref(), Some("3.6a"));
        assert_eq!(row.last_pinged_at, Some(1000));
    }

    #[test]
    fn delete_host_removes_host_and_its_sessions() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 1);
        s.delete_host("h").unwrap();
        assert_eq!(
            s.list_hosts()
                .unwrap()
                .iter()
                .filter(|x| x.alias == "h")
                .count(),
            0
        );
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 0);
    }

    #[test]
    fn delete_host_refuses_to_remove_local() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.delete_host("local").unwrap();
        assert!(s.list_hosts().unwrap().iter().any(|h| h.alias == "local"));
    }

    #[test]
    fn set_host_hidden_toggles() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_hidden("h", true).unwrap();
        assert!(
            s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
        s.set_host_hidden("h", false).unwrap();
        assert!(
            !s.list_hosts()
                .unwrap()
                .iter()
                .find(|x| x.alias == "h")
                .unwrap()
                .hidden
        );
    }

    #[test]
    fn host_provisioned_defaults_false_and_round_trips() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        assert!(!s.get_host_row("local").unwrap().unwrap().provisioned);
        s.set_host_provisioned("local", true).unwrap();
        assert!(s.get_host_row("local").unwrap().unwrap().provisioned);
    }

    #[test]
    fn upsert_account_inserts_then_updates_keeping_uuid_pk() {
        let s = Store::open_in_memory().unwrap();
        let a = AccountRow {
            uuid: "uuid-1".into(),
            email: Some("a@b.com".into()),
            display_name: Some("A".into()),
            organization_name: None,
            organization_uuid: None,
            seat_tier: Some("max".into()),
            last_seen_at: Some(1000),
        };
        s.upsert_account(&a).unwrap();
        let mut a2 = a.clone();
        a2.email = Some("a@c.com".into());
        a2.last_seen_at = Some(2000);
        s.upsert_account(&a2).unwrap();
        let listed = s.list_accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].uuid, "uuid-1");
        assert_eq!(listed[0].email.as_deref(), Some("a@c.com"));
        assert_eq!(listed[0].last_seen_at, Some(2000));
    }

    #[test]
    fn list_accounts_orders_by_uuid_ascending() {
        let s = Store::open_in_memory().unwrap();
        for uuid in ["zzz", "aaa", "mmm"] {
            s.upsert_account(&AccountRow {
                uuid: uuid.into(),
                email: None,
                display_name: None,
                organization_name: None,
                organization_uuid: None,
                seat_tier: None,
                last_seen_at: None,
            })
            .unwrap();
        }
        let listed = s.list_accounts().unwrap();
        assert_eq!(
            listed.iter().map(|a| a.uuid.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "mmm", "zzz"]
        );
    }

    #[test]
    fn get_account_by_uuid_returns_none_when_missing() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.get_account_by_uuid("nope").unwrap().is_none());
    }

    #[test]
    fn get_account_by_uuid_returns_some_when_present() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: Some("x@y.com".into()),
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        let got = s.get_account_by_uuid("u1").unwrap().unwrap();
        assert_eq!(got.email.as_deref(), Some("x@y.com"));
    }

    #[test]
    fn set_host_account_assigns_and_clears() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        s.set_host_account("h", Some("u1")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("u1"));
        s.set_host_account("h", None).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn list_hosts_includes_account_uuid_in_output() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|r| r.alias == "h")
            .unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn get_session_account_returns_none_for_missing_then_some_after_upsert() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // No session yet → None
        assert!(s.get_session_account("h", "dev-foo").unwrap().is_none());
        // Upsert with an account uuid
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        s.upsert_session("dev-foo", "h", None, None, 1, 1, "running", Some("u1"))
            .unwrap();
        assert_eq!(
            s.get_session_account("h", "dev-foo").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[test]
    fn related_matches_same_project_and_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tmux_name, "b");
    }

    #[test]
    fn related_excludes_different_worktree_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("feat-x")).unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn related_matches_across_hosts_same_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let b = store
            .upsert_session("b", "mefistos", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        store.set_worktree_key(a, Some("main")).unwrap();
        store.set_worktree_key(b, Some("main")).unwrap();
        let r = store.list_related_sessions(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].host_alias, "mefistos");
    }

    #[test]
    fn related_returns_empty_for_null_key() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let a = store
            .upsert_session("a", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let _b = store
            .upsert_session("b", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        assert!(store.list_related_sessions(a).unwrap().is_empty());
    }

    #[test]
    fn with_snapshot_returns_owned_data_for_off_lock_use() {
        let store = Store::open_in_memory().expect("in-memory store");
        store
            .insert_host("alpha", Some("alpha-ssh"))
            .expect("insert");
        let hosts = store.with_snapshot(|s| s.list_hosts().expect("list"));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "alpha");
    }

    #[test]
    fn with_transaction_commits_on_ok() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["foo", "foo-ssh"],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        let hosts = store.list_hosts().expect("list");
        assert!(hosts.iter().any(|h| h.alias == "foo"));
    }

    #[test]
    fn with_transaction_rolls_back_on_err() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["bar", "bar-ssh"],
            )?;
            // Trigger an error to force rollback.
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(r.is_err());
        let hosts = store.list_hosts().expect("list");
        assert!(
            !hosts.iter().any(|h| h.alias == "bar"),
            "rollback should have removed the bar row"
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

    #[test]
    fn list_related_sessions_excludes_orphans() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s
            .upsert_session("dev-a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let _b = s
            .upsert_session("dev-b", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert!(
            related.is_empty(),
            "orphans should not match each other; got: {:?}",
            related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn store_holds_event_bus_field_and_default_is_noop() {
        use crate::events::NoopEventBus;
        let store = Store::open_in_memory().expect("store");
        // Just constructing the store with the default Noop bus exercises the
        // new field. The bus is a private implementation detail; we don't expose
        // it as a public getter, so this test is intentionally minimal.
        let _ = std::sync::Arc::new(NoopEventBus); // also exercises Send+Sync
        let _ = store; // touch it to keep it alive past the new
    }

    fn store_with_recorder() -> (Store, Arc<crate::events::RecordingEventBus>) {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
        (store, bus)
    }

    #[test]
    fn upsert_session_emits_created_then_updated() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 100, 200, "running", None)
            .unwrap();
        let evts = bus.take();
        assert_eq!(
            evts.len(),
            2,
            "expected one created + one updated, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:created:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:updated:"), "got: {}", evts[1]);
    }

    #[test]
    fn delete_session_emits_killed() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 100, 100, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().expect("row").id;
        bus.take(); // drain created event
        store.delete_session(id).unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0], format!("session:killed:{id}"));
    }

    #[test]
    fn delete_sessions_not_in_emits_killed_per_row() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain creates
        store
            .delete_sessions_not_in("alpha", &["s2".to_string()])
            .unwrap();
        let evts = bus.take();
        assert_eq!(evts.len(), 2, "expected 2 killed (s1, s3), got {evts:?}");
        assert!(evts.iter().all(|e| e.starts_with("session:killed:")));
    }

    #[test]
    fn delete_host_emits_session_killed_per_orphaned_session() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("s2", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        bus.take(); // drain host:added + 2x session:created
        store.delete_host("alpha").unwrap();
        let evts = bus.take();
        // Expected order: 2x session:killed (one per orphan), then host:removed.
        assert_eq!(
            evts.len(),
            3,
            "expected 2 session:killed + 1 host:removed, got {evts:?}"
        );
        assert!(evts[0].starts_with("session:killed:"), "got: {}", evts[0]);
        assert!(evts[1].starts_with("session:killed:"), "got: {}", evts[1]);
        assert_eq!(evts[2], "host:removed:alpha");
    }

    #[test]
    fn restore_session_clears_ghost_status_and_lost_at() {
        let (store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        // Manually ghost it
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
                rusqlite::params![id],
            )
            .unwrap();
        bus.take(); // drain

        let row = store.restore_session(id).unwrap().expect("row must exist");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "restore must emit session:updated; got: {evts:?}"
        );
    }

    #[test]
    fn migration_005_adds_kind_and_reviews_columns_with_defaults() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rows = store.list_sessions_for_host("alpha").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "work");
        assert_eq!(rows[0].reviews_session_id, None);
    }

    #[test]
    fn migration_008_adds_lost_at_column() {
        let store = Store::open_in_memory().expect("store");
        let v: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 16, "schema_version should be 16 after migration");
        // Column exists and defaults to NULL
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let lost: Option<i64> = store
            .conn
            .query_row(
                "SELECT lost_at FROM sessions WHERE tmux_name='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lost, None, "lost_at should be NULL for a fresh session");
    }

    #[test]
    fn mark_sessions_reconciled_stamps_only_kept_rows() {
        // Task H freshness marker: the background tick stamps last_reconciled_at
        // on every session it observed live (the keep set) and leaves the rest
        // (and a fresh row's default) NULL.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("kept", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("gone", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        let updated = store
            .mark_sessions_reconciled("alpha", &["kept".to_string()], 1234)
            .expect("mark");
        assert_eq!(updated, 1, "only the kept session is stamped");

        let kept_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='kept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept_at, Some(1234));

        let gone_at: Option<i64> = store
            .conn
            .query_row(
                "SELECT last_reconciled_at FROM sessions WHERE tmux_name='gone'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone_at, None, "non-kept session keeps NULL");

        // Empty keep set is a no-op (no rows touched, no error).
        let none = store
            .mark_sessions_reconciled("alpha", &[], 9999)
            .expect("empty keep");
        assert_eq!(none, 0);
    }

    #[test]
    fn set_session_kind_marks_review_and_survives_reupsert() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        let src = store
            .upsert_session("src", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rev = store
            .upsert_session("src--review-1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_session_kind(rev, "review", Some(src)).unwrap();
        store
            .upsert_session("src--review-1", "alpha", None, None, 1, 2, "running", None)
            .unwrap();
        let row = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .find(|r| r.tmux_name == "src--review-1")
            .unwrap();
        assert_eq!(row.kind, "review", "kind must survive re-upsert");
        assert_eq!(row.reviews_session_id, Some(src));
    }

    #[test]
    fn reconcile_clears_stuck_kind_only_when_pane_observed() {
        let (mut store, _bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();

        let pass = |store: &mut Store, stuck: Option<&str>, observed: bool| {
            let sessions = vec![ReconcileSession {
                tmux_name: "s1",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: stuck.map(|s| s.to_string()),
                intel_observed: observed,
            }];
            store
                .apply_host_reconcile(HostReconcile {
                    alias: "alpha",
                    reachable: true,
                    claude_version: None,
                    tmux_version: None,
                    last_pinged_at: 1,
                    sessions: &sessions,
                    keep: &["s1".to_string()],
                })
                .unwrap();
        };
        let stuck_of = |store: &Store| {
            store
                .get_session("s1", "alpha")
                .unwrap()
                .unwrap()
                .stuck_kind
        };

        // Observed pane, stuck detected → flag stored.
        pass(&mut store, Some("reconnect"), true);
        assert_eq!(stuck_of(&store).as_deref(), Some("reconnect"));

        // Capture FAILED (pane not observed), no stuck → prior flag preserved.
        pass(&mut store, None, false);
        assert_eq!(
            stuck_of(&store).as_deref(),
            Some("reconnect"),
            "must preserve stuck_kind when the pane was not observed"
        );

        // Observed pane, stuck no longer present → flag CLEARED.
        pass(&mut store, None, true);
        assert_eq!(
            stuck_of(&store),
            None,
            "must clear stuck_kind when the pane was observed and shows no stuck state"
        );
    }

    #[test]
    fn apply_host_reconcile_happy_path_persists_all_and_emits_after_commit() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        // Pre-seed a stale row that should be pruned (kill), and one that
        // already exists so it produces an `updated` (not `created`).
        store
            .upsert_session("stale", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("keep-existing", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let stale_id = store.get_session("stale", "alpha").unwrap().unwrap().id;
        bus.take(); // drain all setup events

        let sessions = vec![
            // existing → update
            ReconcileSession {
                tmux_name: "keep-existing",
                project_id: Some(pid),
                created_at: 1,
                last_activity_at: 50,
                account_uuid: None,
                worktree_key: Some("main".to_string()),
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
            },
            // brand new → create
            ReconcileSession {
                tmux_name: "fresh",
                project_id: Some(pid),
                created_at: 10,
                last_activity_at: 60,
                account_uuid: None,
                worktree_key: Some("main".to_string()),
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
            },
        ];
        let keep = vec!["keep-existing".to_string(), "fresh".to_string()];
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: Some("2.1"),
                tmux_version: Some("3.6"),
                last_pinged_at: 999,
                sessions: &sessions,
                keep: &keep,
            })
            .expect("reconcile ok");

        // (a) rows persisted: stale ghosted, two live, host probe updated.
        let live: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status != "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(live, vec!["fresh", "keep-existing"], "two live sessions");
        let ghosts: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .filter(|r| r.status == "ghost")
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(ghosts, vec!["stale"], "stale is now ghost");
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_eq!(host.claude_version.as_deref(), Some("2.1"));
        assert_eq!(host.last_pinged_at, Some(999));
        assert_eq!(store.list_projects().unwrap()[0].last_session_at, Some(60));

        // (b) the events fired — and only after commit (we drained pre-batch, so
        // everything here was emitted by the flush phase).
        let evts = bus.take();
        assert!(
            evts.contains(&"host:probed:alpha".to_string()),
            "got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "expected an update for keep-existing; got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("session:created:")),
            "expected a create for fresh; got: {evts:?}"
        );
        assert!(
            evts.contains(&format!("session:updated:{stale_id}")),
            "stale becomes ghost (session:updated); got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("project:updated:")),
            "expected project:updated; got: {evts:?}"
        );
    }

    #[test]
    fn bg_session_survives_reconcile_with_empty_tmux() {
        // A `kind='bg'` row is never a tmux session, so it never appears in the
        // `keep` set. Ghost cleanup must NOT reap it — even when the host's tmux
        // list is empty and a normal (work) row gets ghosted.
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        // A bg row + a normal work row.
        store
            .upsert_bg_session(
                "alpha",
                "bg:sess-uuid-1",
                None,
                "sess-uuid-1",
                Some("working"),
                100,
            )
            .unwrap();
        store
            .upsert_session("work-a", "alpha", None, None, 1, 1, "running", None)
            .unwrap();

        // Reconcile with NO live tmux sessions (empty keep).
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 1,
                sessions: &[],
                keep: &[],
            })
            .expect("reconcile ok");

        let rows = store.list_sessions_for_host("alpha").unwrap();
        let bg = rows
            .iter()
            .find(|r| r.tmux_name == "bg:sess-uuid-1")
            .expect("bg row must survive");
        assert_eq!(bg.kind, "bg");
        assert_eq!(bg.status, "running", "bg row must NOT be ghosted");
        assert_eq!(bg.claude_session_id.as_deref(), Some("sess-uuid-1"));
        // The plain work row, in contrast, gets ghosted.
        let work = rows.iter().find(|r| r.tmux_name == "work-a").unwrap();
        assert_eq!(work.status, "ghost", "work row IS ghosted when not in tmux");

        // A SECOND reconcile (the bg row is now an old row) still doesn't reap
        // it via the Phase-2 hard-delete.
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 2,
                sessions: &[],
                keep: &[],
            })
            .expect("reconcile ok");
        assert!(
            store
                .list_sessions_for_host("alpha")
                .unwrap()
                .iter()
                .any(|r| r.tmux_name == "bg:sess-uuid-1"),
            "bg row must survive repeated reconciles"
        );
    }

    #[test]
    fn reconcile_batch_rolls_back_and_emits_nothing_on_error() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        bus.take(); // drain host:added

        let row_count = |s: &Store| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE host_alias='alpha'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(row_count(&store), 0);

        // First a good upsert, then one whose project_id points at a
        // non-existent project — with foreign_keys=ON this trips the
        // sessions.project_id FK mid-batch and aborts the transaction.
        let sessions = vec![
            ReconcileSession {
                tmux_name: "good",
                project_id: None,
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
            },
            ReconcileSession {
                tmux_name: "bad",
                project_id: Some(999_999), // no such project → FK violation
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
                claude_session_id: None,
                claude_status: None,
                effort_level: None,
                pr_url: None,
                current_activity: None,
                context_pct: None,
                stuck_kind: None,
                intel_observed: false,
            },
        ];
        let keep = vec!["good".to_string(), "bad".to_string()];
        let res = store.apply_host_reconcile(HostReconcile {
            alias: "alpha",
            reachable: true,
            claude_version: Some("9.9"),
            tmux_version: None,
            last_pinged_at: 12345,
            sessions: &sessions,
            keep: &keep,
        });

        assert!(res.is_err(), "FK violation should abort the batch");
        // (a) NO rows persisted — not even the 'good' one before the failure.
        assert_eq!(
            row_count(&store),
            0,
            "transaction must have rolled back all writes"
        );
        // host probe row must also be untouched (it was part of the same tx).
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "alpha")
            .unwrap();
        assert_ne!(
            host.claude_version.as_deref(),
            Some("9.9"),
            "host probe rolled back"
        );
        assert_eq!(host.last_pinged_at, None, "host probe rolled back");
        // (b) NO events emitted — the flush phase never runs on rollback.
        assert!(
            bus.take().is_empty(),
            "no event may fire for a rolled-back batch"
        );
    }

    #[test]
    fn reconcile_ghosts_sessions_on_first_empty_probe_then_deletes_on_second() {
        let (mut store, bus) = store_with_recorder();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 10, "running", None)
            .unwrap();
        let s1_id = store.get_session("s1", "alpha").unwrap().unwrap().id;
        bus.take(); // drain setup events

        // First reachable probe with no sessions — s1 should become ghost
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 100,
                sessions: &[],
                keep: &[],
            })
            .unwrap();

        let s1 = store.get_session_by_id(s1_id).unwrap().unwrap();
        assert_eq!(s1.status, "ghost", "first empty probe should ghost s1");
        assert!(s1.lost_at.is_some(), "lost_at must be set");

        let evts = bus.take();
        assert!(
            evts.iter().any(|e| e.starts_with("session:updated:")),
            "ghost transition should emit session:updated; got: {evts:?}"
        );
        assert!(
            !evts.iter().any(|e| e.starts_with("session:killed:")),
            "no kill event on first cycle; got: {evts:?}"
        );

        // Second reachable probe with no sessions — ghost s1 should be deleted
        store
            .apply_host_reconcile(HostReconcile {
                alias: "alpha",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: 200,
                sessions: &[],
                keep: &[],
            })
            .unwrap();

        assert!(
            store.get_session_by_id(s1_id).unwrap().is_none(),
            "second empty probe should hard-delete the ghost"
        );
        let evts2 = bus.take();
        assert!(
            evts2.contains(&format!("session:killed:{s1_id}")),
            "second cycle must emit session:killed; got: {evts2:?}"
        );
    }

    #[test]
    fn claude_session_id_round_trips_and_defaults_none() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id,
            None
        );
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_claude_session_id() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }
}
