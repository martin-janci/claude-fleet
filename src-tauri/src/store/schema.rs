//! Schema migrations: the ordered `MIGRATIONS` list, `migrate`, and the
//! guards that let a migration run over an existing database.

use super::*;

/// One `PRAGMA foreign_key_check` row: a child row whose foreign key names a
/// missing parent. `(table, rowid, parent, fkid)` identifies it stably across
/// a migration: rowids survive the 024 rebuild (the id column is the rowid)
/// and the rebuilt table declares its foreign keys in the same order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FkViolation {
    pub table: String,
    pub rowid: Option<i64>,
    pub parent: String,
    pub fkid: i64,
}

/// Every dangling foreign key in the database.
fn fk_violations(conn: &Connection) -> rusqlite::Result<Vec<FkViolation>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let rows = stmt.query_map([], |r| {
        Ok(FkViolation {
            table: r.get(0)?,
            rowid: r.get(1)?,
            parent: r.get(2)?,
            fkid: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// `already_applied` guard of migration 024: `worktrees` already has its
/// `host_alias` column. 024 rebuilds the table, and running it again would
/// reset every row's host to 'local' (and collide remote rows with
/// same-named local ones), so on such a table a re-run only records the
/// version. See [`Migration`].
fn worktrees_has_host_alias(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('worktrees') WHERE name = 'host_alias'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 025 (session usage): it adds its
/// columns in one transaction, so its last column present means the whole
/// migration is, and a re-run (`ALTER TABLE ... ADD COLUMN` again) would
/// fail. See [`Migration`].
fn sessions_has_usage_columns(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') \
         WHERE name = 'usage_last_msg_usage'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 026: `worktrees` already has its
/// `updated_at_ms` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn worktrees_has_updated_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('worktrees') WHERE name = 'updated_at_ms'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Ordered schema migrations, `(version, sql)`. Versions are contiguous from
/// 1 and every script must end by recording its own version with
/// `INSERT OR IGNORE INTO schema_version (version) VALUES (N)` — the tests
/// enforce both. `migrate()` runs entry 0 unconditionally (it is idempotent
/// and bootstraps `schema_version`) and each later entry in its own
/// transaction iff its version is above the recorded maximum. To add one:
/// drop `NNN_name.sql` into `migrations/` and append it here.
const MIGRATIONS: &[Migration] = &[
    Migration::plain(1, include_str!("../../migrations/001_init.sql")),
    Migration::plain(2, include_str!("../../migrations/002_hosts_ssh.sql")),
    Migration::plain(3, include_str!("../../migrations/003_accounts.sql")),
    Migration::plain(4, include_str!("../../migrations/004_session_account.sql")),
    Migration::plain(5, include_str!("../../migrations/005_session_reviews.sql")),
    Migration::plain(
        6,
        include_str!("../../migrations/006_session_worktree_key.sql"),
    ),
    Migration::plain(7, include_str!("../../migrations/007_indexes.sql")),
    Migration::plain(8, include_str!("../../migrations/008_ghost_sessions.sql")),
    Migration::plain(
        9,
        include_str!("../../migrations/009_session_claude_id.sql"),
    ),
    Migration::plain(
        10,
        include_str!("../../migrations/010_claude_agent_fields.sql"),
    ),
    Migration::plain(
        11,
        include_str!("../../migrations/011_host_provisioned.sql"),
    ),
    Migration::plain(
        12,
        include_str!("../../migrations/012_session_context_pressure.sql"),
    ),
    Migration::plain(13, include_str!("../../migrations/013_session_events.sql")),
    Migration::plain(
        14,
        include_str!("../../migrations/014_last_reconciled_at.sql"),
    ),
    Migration::plain(
        15,
        include_str!("../../migrations/015_session_messages.sql"),
    ),
    Migration::plain(
        16,
        include_str!("../../migrations/016_session_friendly_name.sql"),
    ),
    Migration::plain(17, include_str!("../../migrations/017_safe_kill.sql")),
    Migration::plain(18, include_str!("../../migrations/018_host_tokens.sql")),
    Migration::plain(
        19,
        include_str!("../../migrations/019_lifecycle_fields.sql"),
    ),
    Migration::plain(20, include_str!("../../migrations/020_tasks_and_turns.sql")),
    Migration::plain(21, include_str!("../../migrations/021_repair_backoff.sql")),
    Migration::plain(
        22,
        include_str!("../../migrations/022_drop_handoff_freeze.sql"),
    ),
    Migration::plain(
        23,
        include_str!("../../migrations/023_worktree_parent_fingerprints.sql"),
    ),
    // A table rebuild cannot be written idempotently in SQL, and re-running
    // it would reset every row's host to 'local'.
    Migration {
        version: 24,
        sql: include_str!("../../migrations/024_worktree_host.sql"),
        already_applied: Some(worktrees_has_host_alias),
    },
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 25,
        sql: include_str!("../../migrations/025_session_usage.sql"),
        already_applied: Some(sessions_has_usage_columns),
    },
    Migration {
        version: 26,
        sql: include_str!("../../migrations/026_worktree_updated_at.sql"),
        already_applied: Some(worktrees_has_updated_at),
    },
];

/// One schema migration. `already_applied`, when set, reports whether the
/// migration's change is already in the schema, for a migration that cannot
/// be written idempotently in SQL. Tests roll the recorded version back and
/// migrate again, which re-runs every later migration; such a migration is
/// then only recorded, not re-run. `None` (the usual case) always runs it.
#[derive(Clone, Copy)]
struct Migration {
    version: i64,
    sql: &'static str,
    already_applied: Option<fn(&Connection) -> rusqlite::Result<bool>>,
}

impl Migration {
    /// A migration that is safe to run as written.
    const fn plain(version: i64, sql: &'static str) -> Self {
        Migration {
            version,
            sql,
            already_applied: None,
        }
    }
}

/// The schema version a fully migrated database reports.
#[cfg(test)]
const LATEST_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].version;

impl Store {
    pub(super) fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        // The bootstrap migration is idempotent (`CREATE TABLE IF NOT EXISTS`
        // + `INSERT OR IGNORE`) and always runs: on a fresh DB it also creates
        // `schema_version`, which the gate below needs to exist.
        let bootstrap = MIGRATIONS[0].sql;
        self.conn.execute_batch(bootstrap)?;
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
        //
        // Pending migrations run with foreign keys OFF. A table rebuild (024
        // changes `worktrees`' UNIQUE key) must drop a parent table that
        // `sessions.worktree_id` references, which SQLite only allows that
        // way. The pragma is a no-op inside a transaction, so it is set here,
        // outside them; `apply_migrations` then refuses to commit any
        // migration that ADDS a dangling reference, and foreign keys go back
        // on even when a migration fails. Dangling references that were
        // already there (a hand edit in the sqlite3 CLI, where foreign keys
        // default to off, can leave one) are logged and never fatal: failing
        // on them would stop the app from starting with no way out short of
        // manual SQL.
        let pending: Vec<Migration> = MIGRATIONS
            .iter()
            .copied()
            .filter(|m| m.version > v)
            .collect();
        if !pending.is_empty() {
            self.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
            let applied = self.apply_migrations(&pending);
            let restored = self.conn.execute_batch("PRAGMA foreign_keys = ON;");
            for violation in applied? {
                tracing::warn!(
                    "migration left a pre-existing dangling foreign key in place: {violation:?}"
                );
            }
            restored?;
        }
        self.reap_orphan_session_events()?;
        Ok(())
    }

    /// Run `pending` migrations in order, each in its own transaction, and
    /// return the dangling references that were ALREADY in the database.
    /// `PRAGMA foreign_key_check` covers the whole database, so it is taken
    /// before and after each migration: only rows the migration added roll it
    /// back (SQLite's table-rebuild procedure); rows present before it are
    /// left alone and returned for the caller to log.
    fn apply_migrations(&self, pending: &[Migration]) -> Result<Vec<FkViolation>> {
        let mut preexisting: Vec<FkViolation> = Vec::new();
        for &Migration {
            version,
            sql,
            already_applied,
        } in pending
        {
            let tx = self.conn.unchecked_transaction()?;
            if already_applied
                .map(|applied| applied(&tx))
                .transpose()?
                .unwrap_or(false)
            {
                // Re-run on a schema that already has this change (a test
                // rolled the recorded version back): only record it.
                tx.execute(
                    "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
                    rusqlite::params![version],
                )?;
                tx.commit()?;
                continue;
            }
            let before: std::collections::HashSet<FkViolation> =
                fk_violations(&tx)?.into_iter().collect();
            tx.execute_batch(sql)?;
            let mut added: Vec<FkViolation> = Vec::new();
            for violation in fk_violations(&tx)? {
                if !before.contains(&violation) {
                    added.push(violation);
                } else if !preexisting.contains(&violation) {
                    preexisting.push(violation);
                }
            }
            if let Some(v) = added.first() {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                    Some(format!(
                        "migration {version} left a dangling foreign key: {} row {:?} -> {} (fk {})",
                        v.table, v.rowid, v.parent, v.fkid
                    )),
                ));
            }
            tx.commit()?;
        }
        Ok(preexisting)
    }

    /// Drop `session_events` rows whose session no longer exists. Deletes that
    /// predate `delete_session` reaping the timeline left such orphans behind;
    /// a reused session id would inherit them. Idempotent, runs on open.
    fn reap_orphan_session_events(&self) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM session_events \
             WHERE NOT EXISTS (SELECT 1 FROM sessions WHERE sessions.id = session_events.session_id)",
            [],
        )
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
        "settings",
        "schema_version",
        "session_events",
        "session_messages",
        "host_tokens",
        "tasks",
        "worktree_parent_fingerprints",
    ];

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in EXPECTED_TABLES {
            assert!(store.has_table(t).expect("has_table"), "missing table: {t}");
        }
    }

    fn session_columns(s: &Store) -> Vec<String> {
        let mut stmt = s.conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let mut out = Vec::new();
        for c in stmt.query_map([], |r| r.get::<_, String>(1)).unwrap() {
            out.push(c.unwrap());
        }
        out
    }

    /// The handoff/freeze drop (W5 G2) on an existing database at version
    /// 20 that still has the dead `handoffs` table (with a row) and
    /// `sessions.frozen_scrollback` (with a value): every later migration
    /// applies cleanly, both are gone, the session row survives, and a
    /// relaunch — which re-runs the 001 bootstrap — is a strict no-op and
    /// does not bring the table back.
    #[test]
    fn handoff_freeze_drop_applies_on_an_existing_v20_db_and_is_idempotent() {
        const SEED_AT: i64 = 20;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= SEED_AT) {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }
        // The table exactly as the pre-022 bootstrap created it.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS handoffs (
               id INTEGER PRIMARY KEY,
               session_id INTEGER NOT NULL REFERENCES sessions(id),
               from_host TEXT NOT NULL, to_host TEXT NOT NULL, mode TEXT NOT NULL,
               started_at INTEGER NOT NULL, finished_at INTEGER,
               status TEXT NOT NULL, error TEXT);
             INSERT INTO hosts (alias) VALUES ('h');
             INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status, frozen_scrollback)
               VALUES ('s', 'h', 1, 1, 'running', 'frozen text');
             INSERT INTO handoffs (session_id, from_host, to_host, mode, started_at, status)
               VALUES (1, 'h', 'x', 'mirror', 1, 'done');",
        )
        .unwrap();
        let store = Store {
            conn,
            bus: Arc::new(NoopEventBus),
        };
        assert!(store.has_table("handoffs").unwrap());
        assert!(session_columns(&store).contains(&"frozen_scrollback".to_string()));
        assert_eq!(store.schema_version().unwrap(), SEED_AT);

        store.migrate().expect("migrate a v20 db");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
        let row = store.get_session("s", "h").unwrap().expect("row survives");
        assert_eq!(row.status, "running");
        assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        // Relaunch: the bootstrap re-runs; nothing changes, the table stays gone.
        let objects = |s: &Store| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let before = (objects(&store), session_columns(&store));
        store.migrate().expect("second migrate");
        assert!(!store.has_table("handoffs").unwrap());
        assert_eq!((objects(&store), session_columns(&store)), before);
        assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// A fresh database: 001 still lists `frozen_scrollback` (so the drop
    /// can be an unconditional DROP COLUMN) and no longer creates
    /// `handoffs`; after the full migrate neither exists, and a second
    /// migrate keeps it that way.
    #[test]
    fn fresh_db_has_no_handoffs_or_frozen_scrollback() {
        let store = Store::open_in_memory().expect("open");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
        store.migrate().expect("second migrate");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(
            store.schema_version().expect("version"),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrations_are_contiguous_from_one() {
        assert!(!MIGRATIONS.is_empty());
        for (i, &Migration { version, .. }) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                version,
                i as i64 + 1,
                "MIGRATIONS[{i}] has version {version}"
            );
        }
        assert_eq!(LATEST_SCHEMA_VERSION, MIGRATIONS.len() as i64);
    }

    #[test]
    fn every_migration_records_its_own_version() {
        for &Migration { version, sql, .. } in MIGRATIONS {
            // Normalise whitespace so formatting differences between scripts
            // (line breaks, double spaces) do not matter.
            let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            let stamp =
                format!("INSERT OR IGNORE INTO schema_version (version) VALUES ({version});");
            assert!(
                flat.contains(&stamp),
                "migration {version} must contain `{stamp}`"
            );
            // …and must not stamp any *other* version by mistake.
            let stamps = flat
                .matches("INTO schema_version (version) VALUES (")
                .count();
            assert_eq!(stamps, 1, "migration {version} stamps {stamps} versions");
        }
    }

    #[test]
    fn second_migrate_on_fresh_db_is_a_noop() {
        let store = Store::open_in_memory().expect("open");
        let before = store.schema_version().expect("version");
        let count = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
                .unwrap()
        };
        let rows_before = count(&store);
        let tables_before = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        store.migrate().expect("second migrate");
        assert_eq!(store.schema_version().expect("version"), before);
        assert_eq!(count(&store), rows_before);
        let tables_after = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(tables_after, tables_before);
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
    fn schema_version_is_latest_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), LATEST_SCHEMA_VERSION);
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
    fn open_reaps_orphaned_session_events_idempotently() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let live = store
            .upsert_session("live", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(live, "status_change", Some("idle"))
            .unwrap();
        // Left behind by a delete that predates the reap in delete_session.
        store
            .insert_session_event(live + 1000, "killed", None)
            .unwrap();

        store.migrate().unwrap();
        store.migrate().unwrap();

        assert_eq!(store.list_session_events(live, 10).unwrap().len(), 1);
        assert!(store
            .list_session_events(live + 1000, 10)
            .unwrap()
            .is_empty());
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

    /// A database that stopped at `version`: the migrations up to it, with
    /// foreign keys on as a real install ran them.
    fn store_at_version(version: i64) -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= version) {
            conn.execute_batch(sql).unwrap();
        }
        Store {
            conn,
            bus: Arc::new(NoopEventBus),
        }
    }

    #[test]
    fn migration_024_fresh_db_has_host_scoped_worktrees() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let local = s
            .upsert_worktree(pid, "feat", "/p/r/.worktrees/feat", None)
            .unwrap();
        // Same project and name on another host: its own row, no clash.
        let remote = s
            .upsert_worktree_on(
                "vps",
                pid,
                "feat",
                "/home/u/r/.worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        assert_ne!(local, remote);
        // Upserting again on the same host updates that row in place.
        let again = s
            .upsert_worktree_on("vps", pid, "feat", "/home/u/r/.worktrees/feat2", None)
            .unwrap();
        assert_eq!(again, remote);
        let row = s.get_worktree_row(remote).unwrap().unwrap();
        assert_eq!(
            (row.host_alias.as_str(), row.path.as_str()),
            ("vps", "/home/u/r/.worktrees/feat2")
        );
        assert_eq!(
            s.get_worktree_row(local).unwrap().unwrap().host_alias,
            "local"
        );
        let default: String = s
            .conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('worktrees') WHERE name='host_alias'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(default, "'local'");
    }

    /// 024 on a database with data: every worktree row keeps its id and
    /// becomes `local`, sessions keep their `worktree_id`, foreign keys come
    /// back on (and still guard the rebuilt table), and a second migrate is
    /// a no-op.
    #[test]
    fn migration_024_on_an_existing_db_backfills_local_and_keeps_ids() {
        let old = store_at_version(23);
        old.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('local');
                 INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');
                 INSERT INTO worktrees (id, project_id, name, path, branch)
                   VALUES (7, 1, 'main', '/p/r', 'main');
                 INSERT INTO worktrees (id, project_id, name, path, branch)
                   VALUES (9, 1, 'feat', '/p/r/.worktrees/feat', NULL);
                 INSERT INTO sessions
                   (tmux_name, host_alias, project_id, worktree_id,
                    created_at, last_activity_at, status)
                   VALUES ('dev', 'local', 1, 9, 1, 1, 'running');",
            )
            .unwrap();
        old.migrate().expect("024 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let got: Vec<(i64, String, String)> = old
            .list_worktrees_for_project(1)
            .unwrap()
            .into_iter()
            .map(|w| (w.id, w.host_alias, w.name))
            .collect();
        assert_eq!(
            got,
            vec![
                (9, "local".to_string(), "feat".to_string()),
                (7, "local".to_string(), "main".to_string())
            ]
        );
        let wid: Option<i64> = old
            .conn
            .query_row(
                "SELECT worktree_id FROM sessions WHERE tmux_name='dev'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wid, Some(9));
        let fk: i64 = old
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign keys are back on");
        assert!(
            old.conn
                .execute("DELETE FROM worktrees WHERE id = 9", [])
                .is_err(),
            "the referenced row is still protected"
        );
        // The new key: a remote row may share the local row's name.
        old.upsert_worktree_on("vps", 1, "main", "/home/u/p/r", None)
            .unwrap();
        assert_eq!(old.list_worktrees_for_project(1).unwrap().len(), 2);
        assert_eq!(old.list_worktrees_on_host("vps").unwrap().len(), 1);
        // Double migrate: nothing re-runs, nothing is lost.
        old.migrate().expect("second migrate");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(old.list_worktrees_on_host("vps").unwrap().len(), 1);
        assert_eq!(old.list_worktrees_for_project(1).unwrap().len(), 2);
    }

    /// Pending migrations run with foreign keys off, so the runner itself
    /// must refuse to commit one that leaves a dangling reference.
    #[test]
    fn apply_migrations_rolls_back_a_dangling_foreign_key() {
        let s = Store::open_in_memory().unwrap();
        s.conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        let bad = "INSERT INTO worktrees (project_id, name, path) VALUES (424242, 'x', '/x');";
        let err = s
            .apply_migrations(&[Migration::plain(999, bad)])
            .unwrap_err();
        s.conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        assert!(err.to_string().contains("dangling"), "{err}");
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM worktrees WHERE project_id = 424242",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "the migration rolled back");
    }

    /// The per-entry `already_applied` guard: when it says the change is
    /// already in the schema, the entry is only recorded and its SQL never
    /// runs (here it would fail); when it says no, the SQL runs as usual.
    #[test]
    fn apply_migrations_only_records_an_entry_its_guard_says_is_applied() {
        fn yes(_: &Connection) -> rusqlite::Result<bool> {
            Ok(true)
        }
        fn no(_: &Connection) -> rusqlite::Result<bool> {
            Ok(false)
        }
        let s = Store::open_in_memory().unwrap();
        let guarded = Migration {
            version: 998,
            sql: "THIS IS NOT SQL;",
            already_applied: Some(yes),
        };
        s.apply_migrations(&[guarded])
            .expect("an entry its guard says is applied is not run");
        let stamped: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 998",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stamped, 1, "but its version is recorded");
        let unguarded = Migration {
            version: 997,
            sql: "THIS IS NOT SQL;",
            already_applied: Some(no),
        };
        assert!(
            s.apply_migrations(&[unguarded]).is_err(),
            "a guard that says no runs the SQL"
        );
    }

    /// Seed one dangling reference, as a hand edit in the sqlite3 CLI (foreign
    /// keys default to off there) can leave.
    fn seed_dangling_session(s: &Store) {
        s.conn
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO hosts (alias) VALUES ('local');
                 INSERT INTO sessions
                   (tmux_name, host_alias, project_id, worktree_id,
                    created_at, last_activity_at, status)
                   VALUES ('stale', 'local', NULL, 4242, 1, 1, 'running');
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
    }

    /// `PRAGMA foreign_key_check` covers the whole database: a dangling
    /// reference that was ALREADY there must not block 024 (or brick
    /// startup). It is reported, keyed stably, and left in place; only rows a
    /// migration adds roll it back.
    #[test]
    fn migration_024_applies_over_a_preexisting_dangling_reference() {
        let old = store_at_version(23);
        seed_dangling_session(&old);
        let pending: Vec<Migration> = MIGRATIONS
            .iter()
            .copied()
            .filter(|m| m.version > 23)
            .collect();
        old.conn
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .unwrap();
        let reported = old
            .apply_migrations(&pending)
            .expect("a pre-existing dangling row is not fatal");
        old.conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(
            (reported[0].table.as_str(), reported[0].parent.as_str()),
            ("sessions", "worktrees")
        );
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            old.has_table("worktrees").unwrap()
                && old.list_worktrees_on_host("local").unwrap().is_empty(),
            "the rebuilt table is in place"
        );
        // The app start path too: `migrate()` logs it and succeeds.
        let again = store_at_version(23);
        seed_dangling_session(&again);
        again.migrate().expect("the app still starts");
        assert_eq!(again.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Tests roll the recorded version back and migrate again (#61's upgrade
    /// test does), which re-runs 024. On a table that already has
    /// `host_alias` it must not rebuild again: remote rows keep their host,
    /// even one named like a local row.
    #[test]
    fn migration_024_rerun_keeps_remote_rows() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        s.upsert_worktree(pid, "main", "/p/r", None).unwrap();
        let remote = s
            .upsert_worktree_on("vps", pid, "main", "/home/u/r", None)
            .unwrap();
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 24;")
            .unwrap();
        s.migrate().expect("re-running 024 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.get_worktree_row(remote).unwrap().unwrap().host_alias,
            "vps"
        );
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 1);
    }

    /// Rolling the recorded version back re-runs 025 (session usage); it
    /// must not try to add its columns again, and counted usage survives.
    #[test]
    fn migration_025_rerun_is_a_no_op_and_keeps_usage() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.apply_usage(
            id,
            "local",
            &UsageDelta {
                reset: false,
                totals: UsageTotals {
                    input_tokens: 9,
                    cost_micros: 45,
                    ..Default::default()
                },
                model: None,
                offset: 3,
                source: "x.jsonl".into(),
                last_msg_id: None,
                last_msg_usage: None,
                now: 86_400,
            },
        )
        .unwrap();
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 25;")
            .unwrap();
        s.migrate().expect("re-running 025 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 9);
        assert_eq!(s.usage_daily_since(0, None).unwrap().len(), 1);
    }

    /// 026 on a database with worktree rows (stopped at 025): the column is
    /// added, existing rows keep their data with no stamp (written before
    /// 026), and their next write stamps them.
    #[test]
    fn migration_026_on_an_existing_db_keeps_rows_and_stamps_on_next_write() {
        let old = store_at_version(25);
        old.conn
            .execute_batch(
                "INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');
                 INSERT INTO worktrees (id, project_id, host_alias, name, path, branch)
                   VALUES (7, 1, 'vps', 'feat', '/h/r/feat', 'feat');",
            )
            .unwrap();
        old.migrate().expect("026 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = old.get_worktree_row(7).unwrap().expect("the row survives");
        assert_eq!(
            (
                row.host_alias.as_str(),
                row.path.as_str(),
                row.branch.as_deref()
            ),
            ("vps", "/h/r/feat", Some("feat"))
        );
        assert_eq!(
            old.worktree_updated_at_ms(7).unwrap(),
            None,
            "written before 026: no stamp"
        );
        old.upsert_worktree_on("vps", 1, "feat", "/h/r/feat", Some("feat"))
            .unwrap();
        assert!(
            old.worktree_updated_at_ms(7).unwrap().is_some(),
            "the next write stamps it"
        );
    }

    /// 026 stamps every worktree write, insert and update, with
    /// `updated_at_ms`, and a re-run on a table that already has the column
    /// is only recorded: the stamp survives.
    #[test]
    fn migration_026_stamps_worktree_writes_and_reruns_safely() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let id = s
            .upsert_worktree_on("vps", pid, "feat", "/h/r/feat", None)
            .unwrap();
        assert!(
            s.worktree_updated_at_ms(id)
                .unwrap()
                .is_some_and(|ms| ms > 0),
            "stamped on insert"
        );
        s.set_worktree_updated_at_ms(id, 1).unwrap();
        s.upsert_worktree_on("vps", pid, "feat", "/h/r/feat2", None)
            .unwrap();
        let stamped = s
            .worktree_updated_at_ms(id)
            .unwrap()
            .expect("stamped on update");
        assert!(stamped > 1, "an update restamps: {stamped}");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 26;")
            .unwrap();
        s.migrate().expect("re-running 026 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.worktree_updated_at_ms(id).unwrap(),
            Some(stamped),
            "the stamp survives a re-run"
        );
        assert_eq!(s.worktree_updated_at_ms(999_999).unwrap(), None);
    }

    #[test]
    fn migration_008_adds_lost_at_column() {
        let store = Store::open_in_memory().expect("store");
        let v: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 26, "schema_version should be 26 after migration");
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

    // ── migration 020: orchestration fields, tasks, reply_to ──

    #[test]
    fn migration_020_adds_orchestration_columns_with_defaults() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.turn_seq, 0);
        assert_eq!(row.last_stop_at, None);
        assert_eq!(row.parent_session_id, None);
        assert!(row.tags.is_empty());
        assert!(s.has_table("tasks").unwrap());
    }

    #[test]
    fn migration_023_fresh_db_stores_and_updates_parent_fingerprints() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.has_table("worktree_parent_fingerprints").unwrap());
        assert_eq!(s.parent_fingerprint("local", "/r/w").unwrap(), None);
        s.record_parent_fingerprint("local", "/r/w", "42:7", 1)
            .unwrap();
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:7")
        );
        // Re-recording replaces; other hosts / paths are separate keys.
        s.record_parent_fingerprint("local", "/r/w", "42:9", 2)
            .unwrap();
        s.record_parent_fingerprint("vps", "/r/w", "1:1", 2)
            .unwrap();
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:9")
        );
        assert_eq!(
            s.parent_fingerprint("vps", "/r/w").unwrap().as_deref(),
            Some("1:1")
        );
    }

    #[test]
    fn migration_023_upgrades_an_existing_db_and_keeps_its_rows() {
        // Seed a real v22 database from MIGRATIONS (as the 022 test does), so
        // no later migration ever needs an edit here.
        const SEED_AT: i64 = 22;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= SEED_AT) {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }
        conn.execute_batch(
            "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
             INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
               VALUES ('sess', 'local', 1, 1, 'running');",
        )
        .unwrap();
        let s = Store {
            conn,
            bus: Arc::new(NoopEventBus),
        };
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        assert!(!s.has_table("worktree_parent_fingerprints").unwrap());
        s.migrate().unwrap();
        assert!(s.has_table("worktree_parent_fingerprints").unwrap());
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            s.get_session("sess", "local").unwrap().is_some(),
            "rows survive"
        );
    }

    #[test]
    fn migration_023_double_migrate_keeps_recorded_fingerprints() {
        let s = Store::open_in_memory().unwrap();
        s.record_parent_fingerprint("local", "/r/w", "42:7", 1)
            .unwrap();
        s.migrate().unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:7")
        );
    }

    // ── migration 019: lifecycle + outcome fields ──

    #[test]
    fn migration_019_adds_lifecycle_columns_defaulting_to_null() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.idle_since, None);
        assert_eq!(row.stuck_since, None);
        assert_eq!(row.last_playbook_at, None);
        assert_eq!(row.last_prompt, None);
        assert_eq!(row.started_at, None);
        assert_eq!(row.last_turn_at, None);
        assert_eq!(row.ci_status, None);
    }
}
