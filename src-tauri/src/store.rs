use rusqlite::{Connection, Result};

#[allow(dead_code)]
pub struct Store {
    pub conn: Connection,
}

#[allow(dead_code)]
impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_tables() -> Vec<&'static str> {
        vec![
            "hosts",
            "projects",
            "worktrees",
            "sessions",
            "handoffs",
            "settings",
            "schema_version",
        ]
    }

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in expected_tables() {
            let count: i64 = store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |row| row.get(0),
                )
                .expect("query");
            assert_eq!(count, 1, "missing table: {t}");
        }
    }

    #[test]
    fn schema_version_is_one() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(store.schema_version().expect("version"), 1);
    }
}
