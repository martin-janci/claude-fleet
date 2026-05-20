// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.

use rusqlite::{Connection, Result};

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

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
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
        if v < 2 {
            self.conn
                .execute_batch(include_str!("../migrations/002_hosts_ssh.sql"))?;
        }
        if v < 3 {
            self.conn
                .execute_batch(include_str!("../migrations/003_accounts.sql"))?;
        }
        if v < 4 {
            self.conn
                .execute_batch(include_str!("../migrations/004_session_account.sql"))?;
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
        self.conn.query_row(
            "SELECT id FROM projects WHERE owner=?1 AND repo=?2",
            rusqlite::params![owner, repo],
            |row| row.get(0),
        )
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
        self.conn.query_row(
            "SELECT id FROM worktrees WHERE project_id=?1 AND name=?2",
            rusqlite::params![project_id, name],
            |row| row.get(0),
        )
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

    pub fn insert_host(
        &self,
        alias: &str,
        ssh_alias: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
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
        Ok(())
    }

    pub fn get_account_by_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<AccountRow>, rusqlite::Error> {
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
        Ok(())
    }

    pub fn delete_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // Sessions are pruned naturally when reconcile_sessions runs against
        // an empty hosts set; we don't cascade here. The `local` host is
        // never removed.
        if alias == "local" {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM sessions WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        self.conn.execute(
            "DELETE FROM hosts WHERE alias=?1",
            rusqlite::params![alias],
        )?;
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
            rusqlite::params![tmux_name, host_alias, project_id, worktree_id,
                              created_at, last_activity_at, status, account_uuid],
        )?;
        self.conn.query_row(
            "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            rusqlite::params![host_alias, tmux_name],
            |row| row.get(0),
        )
    }

    pub fn get_session_account(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
        )?;
        let mut rows = stmt.query_map(
            rusqlite::params![host_alias, tmux_name],
            |row| row.get::<_, Option<String>>(0),
        )?;
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
                    last_activity_at, status, notes, account_uuid
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
            })
        })?;
        rows.collect()
    }

    pub fn list_related_sessions(
        &self,
        session_id: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // Look up source's (project_id, worktree_id) first.
        let (proj, wt): (Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT project_id, worktree_id FROM sessions WHERE id=?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Orphans (project_id=NULL) have no relateds — they share no identity.
        let Some(project_id) = proj else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid
             FROM sessions
             WHERE project_id=?1
               AND ((?2 IS NULL AND worktree_id IS NULL) OR worktree_id=?2)
               AND id<>?3
             ORDER BY host_alias ASC, tmux_name ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id, wt, session_id], |row| {
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
            })
        })?;
        rows.collect()
    }

    pub fn delete_sessions_not_in(
        &self,
        host_alias: &str,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if keep_names.is_empty() {
            return self.conn.execute(
                "DELETE FROM sessions WHERE host_alias=?1",
                rusqlite::params![host_alias],
            );
        }
        let placeholders = keep_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "DELETE FROM sessions WHERE host_alias=?1 AND tmux_name NOT IN ({placeholders})"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
        for n in keep_names {
            params.push(n);
        }
        self.conn.execute(&sql, params.as_slice())
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
        assert_eq!(store.schema_version().expect("version"), 4);
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
    fn schema_version_is_four_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 4);
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
        assert!(s.has_table("accounts").expect("has_table"), "expected accounts table");
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
        let row = s.list_hosts().unwrap().into_iter().find(|x| x.alias == "h").unwrap();
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
        assert_eq!(s.list_hosts().unwrap().iter().filter(|x| x.alias == "h").count(), 0);
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
        assert!(s.list_hosts().unwrap().iter().find(|x| x.alias == "h").unwrap().hidden);
        s.set_host_hidden("h", false).unwrap();
        assert!(!s.list_hosts().unwrap().iter().find(|x| x.alias == "h").unwrap().hidden);
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
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("u1"));
        s.set_host_account("h", None).unwrap();
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
        assert!(row.account_uuid.is_none());
    }

    #[test]
    fn list_hosts_includes_account_uuid_in_output() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        let row = s.list_hosts().unwrap().into_iter().find(|r| r.alias == "h").unwrap();
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
        assert_eq!(s.get_session_account("h", "dev-foo").unwrap().as_deref(), Some("u1"));
    }

    #[test]
    fn list_related_sessions_returns_siblings_with_same_project_and_worktree() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("mefistos").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        let wt1 = s.upsert_worktree(pid, "main", "/tmp/r", Some("main")).unwrap();
        let wt2 = s.upsert_worktree(pid, "feature-x", "/tmp/r/.wt/x", Some("feature-x")).unwrap();
        let a = s.upsert_session("dev-a", "local", Some(pid), Some(wt1), 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "mefistos", Some(pid), Some(wt1), 1, 1, "running", None).unwrap();
        let _c = s.upsert_session("dev-c", "local", Some(pid), Some(wt2), 1, 1, "running", None).unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert_eq!(related.len(), 1, "expected only dev-b as related; got: {:?}", related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>());
        assert_eq!(related[0].tmux_name, "dev-b");
    }

    #[test]
    fn list_related_sessions_matches_null_worktree() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/r").unwrap();
        let wt = s.upsert_worktree(pid, "main", "/tmp/r", Some("main")).unwrap();
        let a = s.upsert_session("dev-a", "h", Some(pid), None, 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "h", Some(pid), None, 1, 1, "running", None).unwrap();
        let _c = s.upsert_session("dev-c", "h", Some(pid), Some(wt), 1, 1, "running", None).unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].tmux_name, "dev-b");
    }

    #[test]
    fn list_related_sessions_excludes_orphans() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s.upsert_session("dev-a", "h", None, None, 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "h", None, None, 1, 1, "running", None).unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert!(related.is_empty(), "orphans should not match each other; got: {:?}", related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>());
    }
}
