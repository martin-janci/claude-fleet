//! The asset catalog's two tables (migration 030): the singleton
//! `catalog_config` row and the per-host/per-harness `asset_inventory`.

use super::*;

impl Store {
    pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT repo_path, remote_url, head_commit, last_loaded_at FROM catalog_config WHERE id = 1",
            )?
            .query_row([], |row| {
                Ok(CatalogConfigRow {
                    repo_path: row.get(0)?,
                    remote_url: row.get(1)?,
                    head_commit: row.get(2)?,
                    last_loaded_at: row.get(3)?,
                })
            })
            .optional()
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
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at, managed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    host_alias, harness, r.kind, r.name, r.state, r.catalog_hash, r.host_hash, r.scanned_at,
                    if r.managed { 1 } else { 0 }
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
            "SELECT host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at, managed
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
                managed: row.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    /// Every known secret name (migration 031): global rows (`host_alias:
    /// None`) first, then per-host overrides. Never carries the value.
    pub fn list_secrets(&self) -> Result<Vec<SecretRow>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name, updated_at FROM catalog_secrets ORDER BY name")?;
        let mut out = stmt
            .query_map([], |row| {
                Ok(SecretRow {
                    name: row.get(0)?,
                    host_alias: None,
                    updated_at: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, name, updated_at FROM catalog_secrets_host ORDER BY host_alias, name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SecretRow {
                host_alias: Some(row.get(0)?),
                name: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        Ok(out)
    }

    /// Resolved secret values for `host_alias`: every global secret,
    /// overlaid by that host's own overrides.
    pub fn secret_values_for_host(
        &self,
        host_alias: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, rusqlite::Error> {
        let pair = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(String, String)> {
            Ok((row.get(0)?, row.get(1)?))
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name, value FROM catalog_secrets")?;
        let mut out = stmt
            .query_map([], pair)?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name, value FROM catalog_secrets_host WHERE host_alias = ?1")?;
        let rows = stmt.query_map(rusqlite::params![host_alias], pair)?;
        out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        Ok(out)
    }

    /// Upsert a secret's value: global (`host_alias: None`) or a per-host
    /// override.
    pub fn set_secret(
        &self,
        name: &str,
        host_alias: Option<&str>,
        value: &str,
    ) -> Result<(), rusqlite::Error> {
        let updated_at = now_unix();
        match host_alias {
            None => {
                self.conn.execute(
                    "INSERT INTO catalog_secrets (name, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(name) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                    rusqlite::params![name, value, updated_at],
                )?;
            }
            Some(host_alias) => {
                self.conn.execute(
                    "INSERT INTO catalog_secrets_host (host_alias, name, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(host_alias, name) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                    rusqlite::params![host_alias, name, value, updated_at],
                )?;
            }
        }
        Ok(())
    }

    /// Delete a secret (global or a per-host override). Returns whether a
    /// row was actually removed.
    pub fn delete_secret(
        &self,
        name: &str,
        host_alias: Option<&str>,
    ) -> Result<bool, rusqlite::Error> {
        let changed = match host_alias {
            None => self.conn.execute(
                "DELETE FROM catalog_secrets WHERE name = ?1",
                rusqlite::params![name],
            )?,
            Some(host_alias) => self.conn.execute(
                "DELETE FROM catalog_secrets_host WHERE host_alias = ?1 AND name = ?2",
                rusqlite::params![host_alias, name],
            )?,
        };
        Ok(changed > 0)
    }

    /// Record a completed sync apply and return its id.
    pub fn record_sync_run(
        &self,
        started_at: i64,
        finished_at: i64,
        summary_json: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO sync_runs (started_at, finished_at, summary_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![started_at, finished_at, summary_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The most recent sync run, if any.
    pub fn last_sync_run(&self) -> Result<Option<SyncRunRow>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT id, started_at, finished_at, summary_json FROM sync_runs ORDER BY id DESC LIMIT 1",
            )?
            .query_row([], |row| {
                Ok(SyncRunRow {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    summary_json: row.get(3)?,
                })
            })
            .optional()
    }

    /// Emit `sync:progress` (not a store row).
    pub fn bus_sync_progress(&self, p: &crate::events::SyncProgress) {
        self.bus.sync_progress(p);
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
            managed: false,
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

    #[test]
    fn secrets_global_and_host_override_resolve_in_order() {
        let s = Store::open_in_memory().unwrap();
        s.set_secret("JIRA_TOKEN", None, "global").unwrap();
        s.set_secret("JIRA_TOKEN", Some("mefistos"), "host")
            .unwrap();
        s.set_secret("OTHER", None, "o").unwrap();
        let local = s.secret_values_for_host("local").unwrap();
        assert_eq!(local["JIRA_TOKEN"], "global");
        assert_eq!(local["OTHER"], "o");
        let mef = s.secret_values_for_host("mefistos").unwrap();
        assert_eq!(mef["JIRA_TOKEN"], "host");
        let names = s.list_secrets().unwrap();
        assert_eq!(names.len(), 3);
        assert!(
            names.iter().all(|r| !format!("{r:?}").contains("global")),
            "list rows must not carry values"
        );
        assert!(s.delete_secret("JIRA_TOKEN", Some("mefistos")).unwrap());
        assert!(!s.delete_secret("JIRA_TOKEN", Some("mefistos")).unwrap());
        assert_eq!(
            s.secret_values_for_host("mefistos").unwrap()["JIRA_TOKEN"],
            "global"
        );
    }

    #[test]
    fn inventory_round_trips_managed_flag() {
        let s = Store::open_in_memory().unwrap();
        let row = AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "s".into(),
            state: "in_sync".into(),
            managed: true,
            ..Default::default()
        };
        s.replace_host_inventory("local", "claude", &[row]).unwrap();
        assert!(s.list_inventory().unwrap()[0].managed);
    }

    #[test]
    fn sync_runs_record_and_last() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.last_sync_run().unwrap().is_none());
        let id = s.record_sync_run(1, 2, "{\"hosts\":[]}").unwrap();
        assert!(id > 0);
        let id2 = s.record_sync_run(3, 4, "{}").unwrap();
        assert_eq!(s.last_sync_run().unwrap().unwrap().id, id2);
    }

    #[test]
    fn bus_sync_progress_records_expected_event() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = Store::open_with_bus_in_memory(dyn_bus).expect("open");
        s.bus_sync_progress(&crate::events::SyncProgress {
            plan_id: "p1".into(),
            host_alias: "local".into(),
            harness: "claude".into(),
            done: 1,
            total: 3,
        });
        assert_eq!(bus.take(), vec!["sync:progress:local:claude:1/3"]);
    }
}
