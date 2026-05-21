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

    // ---- Private fetch helpers used after writes to produce emit payloads ----
    //
    // The row-mapping SQL lives in free `fetch_*` functions that take a bare
    // `&Connection` so it can be reused both by these `&self` helpers AND by
    // the `_in_tx` mutation variants (a `&Transaction` derefs to `&Connection`).

    fn get_session(
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
        let mut stmt = self
            .conn
            .prepare("SELECT id, project_id, name, path, branch FROM worktrees WHERE id=?1")?;
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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

    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn upsert_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, reachable) VALUES (?1, 1)
             ON CONFLICT(alias) DO UPDATE SET reachable=1",
            rusqlite::params![alias],
        )?;
        if let Some(row) = self.get_host(alias)? {
            self.bus.host_added(&row);
        }
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                    last_pinged_at, account_uuid
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
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
            .prepare("SELECT id FROM sessions WHERE host_alias=?1")?
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

        self.conn.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid",
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
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            rusqlite::params![host_alias, tmux_name],
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
        let mut stmt = self
            .conn
            .prepare("SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2")?;
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
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key
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
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key
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

    pub fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                    worktree_key
             FROM sessions WHERE id = ?1",
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
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
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
    // emitting immediately.
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
        status: &str,
        account_uuid: Option<&str>,
        worktree_key: Option<&str>,
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
                                   worktree_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid,
               worktree_key=excluded.worktree_key",
            rusqlite::params![
                tmux_name,
                host_alias,
                project_id,
                worktree_id,
                created_at,
                last_activity_at,
                status,
                account_uuid,
                worktree_key
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

    fn delete_sessions_not_in_in_tx(
        tx: &rusqlite::Transaction,
        host_alias: &str,
        keep_names: &[String],
        out: &mut Vec<RowChange>,
    ) -> Result<usize, rusqlite::Error> {
        // Collect ids to delete before the DELETE so we can emit one event per row.
        let ids_to_delete: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = tx.prepare("SELECT id FROM sessions WHERE host_alias=?1")?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql_select = format!(
                "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = tx.prepare(&sql_select)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        let deleted = if keep_names.is_empty() {
            tx.execute(
                "DELETE FROM sessions WHERE host_alias=?1",
                rusqlite::params![host_alias],
            )?
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            tx.execute(&sql, params.as_slice())?
        };

        for id in &ids_to_delete {
            out.push(RowChange::SessionKilled(*id));
        }
        Ok(deleted)
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
                for sess in spec.sessions {
                    Self::upsert_session_in_tx(
                        tx,
                        sess.tmux_name,
                        spec.alias,
                        sess.project_id,
                        None,
                        sess.created_at,
                        sess.last_activity_at,
                        "running",
                        sess.account_uuid.as_deref(),
                        sess.worktree_key.as_deref(),
                        &mut out,
                    )?;
                    if let Some(pid) = sess.project_id {
                        Self::touch_project_last_session_at_in_tx(
                            tx,
                            pid,
                            sess.last_activity_at,
                            &mut out,
                        )?;
                    }
                }
                Self::delete_sessions_not_in_in_tx(tx, spec.alias, spec.keep, &mut out)?;
            }
            Ok(out)
        })?;

        // Phase 2: transaction committed — now it is safe to emit.
        for change in &changes {
            self.bus.emit_change(change);
        }
        Ok(())
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
        // Collect ids to delete before the DELETE so we can emit one event per row.
        let ids_to_delete: Vec<i64> = if keep_names.is_empty() {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM sessions WHERE host_alias=?1")?;
            let ids = stmt
                .query_map(rusqlite::params![host_alias], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql_select = format!(
                "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            let mut stmt = self.conn.prepare(&sql_select)?;
            let ids = stmt
                .query_map(params.as_slice(), |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        let deleted = if keep_names.is_empty() {
            self.conn.execute(
                "DELETE FROM sessions WHERE host_alias=?1",
                rusqlite::params![host_alias],
            )?
        } else {
            let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            for n in keep_names {
                params.push(n);
            }
            self.conn.execute(&sql, params.as_slice())?
        };

        for id in &ids_to_delete {
            self.bus.session_killed(*id);
        }
        Ok(deleted)
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
    let mut stmt = conn.prepare(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key
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
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_host(conn: &Connection, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden,
                last_pinged_at, account_uuid
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
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn fetch_project(conn: &Connection, id: i64) -> Result<Option<ProjectRow>, rusqlite::Error> {
    let mut stmt = conn
        .prepare("SELECT id, owner, repo, base_path, last_session_at FROM projects WHERE id=?1")?;
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
        assert_eq!(store.schema_version().expect("version"), 6);
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
            .prepare("SELECT name FROM pragma_table_info('hosts')")
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
    fn schema_version_is_six_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 6);
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
            .prepare("SELECT name FROM pragma_table_info('sessions')")
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
            .prepare("SELECT name FROM pragma_table_info('hosts')")
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
            },
            // brand new → create
            ReconcileSession {
                tmux_name: "fresh",
                project_id: Some(pid),
                created_at: 10,
                last_activity_at: 60,
                account_uuid: None,
                worktree_key: Some("main".to_string()),
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

        // (a) rows persisted: stale gone, two live, host probe updated.
        let names: Vec<String> = store
            .list_sessions_for_host("alpha")
            .unwrap()
            .into_iter()
            .map(|r| r.tmux_name)
            .collect();
        assert_eq!(
            names,
            vec!["fresh", "keep-existing"],
            "stale pruned, two live"
        );
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
            evts.contains(&format!("session:killed:{stale_id}")),
            "got: {evts:?}"
        );
        assert!(
            evts.iter().any(|e| e.starts_with("project:updated:")),
            "expected project:updated; got: {evts:?}"
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
            },
            ReconcileSession {
                tmux_name: "bad",
                project_id: Some(999_999), // no such project → FK violation
                created_at: 1,
                last_activity_at: 1,
                account_uuid: None,
                worktree_key: None,
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
}
