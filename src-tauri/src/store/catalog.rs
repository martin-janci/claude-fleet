//! The asset catalog's two tables (migration 030): the singleton
//! `catalog_config` row and the per-host/per-harness `asset_inventory`.

use super::*;

impl Store {
    pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT repo_path, remote_url, head_commit, last_loaded_at FROM catalog_config WHERE id = 1",
        )?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(row) => Ok(Some(CatalogConfigRow {
                repo_path: row.get(0)?,
                remote_url: row.get(1)?,
                head_commit: row.get(2)?,
                last_loaded_at: row.get(3)?,
            })),
            None => Ok(None),
        }
    }

    pub fn set_catalog_config(
        &self,
        repo_path: &str,
        remote_url: Option<&str>,
    ) -> Result<CatalogConfigRow, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO catalog_config (id, repo_path, remote_url) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET repo_path=excluded.repo_path, remote_url=excluded.remote_url,
                                           head_commit=NULL, last_loaded_at=NULL",
            rusqlite::params![repo_path, remote_url],
        )?;
        Ok(self.get_catalog_config()?.expect("row just written"))
    }

    pub fn set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalog_config SET head_commit=?1, last_loaded_at=?2 WHERE id = 1",
            rusqlite::params![head, loaded_at],
        )?;
        Ok(())
    }

    /// Emit `catalog:loaded` (the catalog itself is not a store row).
    pub fn bus_catalog_loaded(&self, summary: &crate::events::CatalogSummary) {
        self.bus.catalog_loaded(summary);
    }

    /// Replace every inventory row for (host, harness) in one transaction,
    /// then emit `asset_inventory:cleared` followed by one `:updated` per row.
    pub fn replace_host_inventory(
        &self,
        host_alias: &str,
        harness: &str,
        rows: &[AssetInventoryRow],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM asset_inventory WHERE host_alias=?1 AND harness=?2",
            rusqlite::params![host_alias, harness],
        )?;
        for r in rows {
            tx.execute(
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    host_alias, harness, r.kind, r.name, r.state, r.catalog_hash, r.host_hash, r.scanned_at
                ],
            )?;
        }
        tx.commit()?;
        self.bus.asset_inventory_cleared(host_alias, harness);
        for r in rows {
            self.bus.asset_inventory_updated(r);
        }
        Ok(())
    }

    pub fn list_inventory(&self) -> Result<Vec<AssetInventoryRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at
             FROM asset_inventory ORDER BY host_alias, harness, kind, name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AssetInventoryRow {
                host_alias: row.get(0)?,
                harness: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                state: row.get(4)?,
                catalog_hash: row.get(5)?,
                host_hash: row.get(6)?,
                scanned_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_config_set_get_and_head() {
        let s = Store::open_in_memory().expect("open");
        assert!(s.get_catalog_config().unwrap().is_none());
        let row = s
            .set_catalog_config("/tmp/assets", Some("git@x:y.git"))
            .unwrap();
        assert_eq!(row.repo_path, "/tmp/assets");
        assert_eq!(row.remote_url.as_deref(), Some("git@x:y.git"));
        assert!(row.head_commit.is_none());
        s.set_catalog_head("abc123", 42).unwrap();
        let row = s.get_catalog_config().unwrap().unwrap();
        assert_eq!(row.head_commit.as_deref(), Some("abc123"));
        assert_eq!(row.last_loaded_at, Some(42));
        // Re-configure replaces path and remote but keeps a single row.
        let row = s.set_catalog_config("/tmp/other", None).unwrap();
        assert_eq!(row.repo_path, "/tmp/other");
        assert!(row.remote_url.is_none());
    }

    #[test]
    fn replace_host_inventory_prunes_and_emits() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = Store::open_with_bus_in_memory(dyn_bus).expect("open");
        let row = |name: &str, state: &str| AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: state.into(),
            catalog_hash: Some("c".into()),
            host_hash: Some("h".into()),
            scanned_at: 1,
        };
        s.replace_host_inventory(
            "local",
            "claude",
            &[row("a", "in_sync"), row("b", "missing")],
        )
        .unwrap();
        assert_eq!(s.list_inventory().unwrap().len(), 2);
        let ev = bus.take();
        assert_eq!(ev[0], "asset_inventory:cleared:local:claude");
        assert!(ev.contains(&"asset_inventory:updated:local:claude:skill:a".to_string()));
        // A second scan that no longer sees `b` prunes it; another host is untouched.
        s.replace_host_inventory(
            "mefistos",
            "claude",
            &[AssetInventoryRow {
                host_alias: "mefistos".into(),
                ..row("z", "drifted")
            }],
        )
        .unwrap();
        s.replace_host_inventory("local", "claude", &[row("a", "drifted")])
            .unwrap();
        let all = s.list_inventory().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|r| r.host_alias == "local" && r.name == "a" && r.state == "drifted"));
        assert!(all
            .iter()
            .any(|r| r.host_alias == "mefistos" && r.name == "z"));
    }
}
