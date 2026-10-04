//! The asset catalog's tables: the `catalogs` table (migration 090, one row
//! per source, `org_id IS NULL` for the personal catalog) plus the
//! per-host/per-harness `asset_inventory` (migration 030). The old
//! singleton `catalog_config` row is no longer read; `get_catalog_config` /
//! `set_catalog_config` / `set_catalog_head` keep their signatures but are
//! now backed by the `personal` row of `catalogs`.

use super::*;

impl Store {
    const CATALOG_COLS: &'static str =
        "id, name, repo_path, remote_url, org_id, head_commit, last_loaded_at";

    fn catalog_row(r: &rusqlite::Row) -> rusqlite::Result<CatalogRow> {
        Ok(CatalogRow {
            id: r.get(0)?,
            name: r.get(1)?,
            repo_path: r.get(2)?,
            remote_url: r.get(3)?,
            org_id: r.get(4)?,
            head_commit: r.get(5)?,
            last_loaded_at: r.get(6)?,
        })
    }

    /// Every catalog: personal first, then the rest by name.
    pub fn list_catalogs(&self) -> Result<Vec<CatalogRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {} FROM catalogs ORDER BY (org_id IS NOT NULL), name",
            Self::CATALOG_COLS
        ))?;
        let rows = stmt.query_map([], Self::catalog_row)?;
        rows.collect()
    }

    pub fn get_catalog(&self, id: i64) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!(
                "SELECT {} FROM catalogs WHERE id = ?1",
                Self::CATALOG_COLS
            ))?
            .query_row([id], Self::catalog_row)
            .optional()
    }

    /// The one catalog with `org_id IS NULL`.
    pub fn personal_catalog(&self) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!(
                "SELECT {} FROM catalogs WHERE org_id IS NULL",
                Self::CATALOG_COLS
            ))?
            .query_row([], Self::catalog_row)
            .optional()
    }

    pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error> {
        Ok(self.personal_catalog()?.map(|c| CatalogConfigRow {
            repo_path: c.repo_path,
            remote_url: c.remote_url,
            head_commit: c.head_commit,
            last_loaded_at: c.last_loaded_at,
        }))
    }

    pub fn set_catalog_config(
        &self,
        repo_path: &str,
        remote_url: Option<&str>,
    ) -> Result<CatalogConfigRow, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO catalogs (name, repo_path, remote_url, org_id, created_at)
             VALUES ('personal', ?1, ?2, NULL, CAST(strftime('%s','now') AS INTEGER))
             ON CONFLICT(name) DO UPDATE SET repo_path=excluded.repo_path, remote_url=excluded.remote_url,
                                             head_commit=NULL, last_loaded_at=NULL",
            rusqlite::params![repo_path, remote_url],
        )?;
        // Migration 093 / Rulings R2: a personal grant made while no personal
        // catalog existed waits in `assets_admin_at`; give it its grant row
        // now that there is a catalog to attach it to. Idempotent. Same
        // transaction as the re-point above: a crash between the two must
        // never leave a personal catalog with its pending grants unmoved.
        tx.execute(
            "INSERT OR IGNORE INTO client_catalog_grants (client_id, catalog_id, granted_at)
             SELECT t.id, c.id, t.assets_admin_at
             FROM client_tokens t JOIN catalogs c ON c.org_id IS NULL
             WHERE t.assets_admin_at IS NOT NULL",
            [],
        )?;
        tx.commit()?;
        Ok(self.get_catalog_config()?.expect("row just written"))
    }

    pub fn get_catalog_by_name(&self, name: &str) -> Result<Option<CatalogRow>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .prepare_cached(&format!(
                "SELECT {} FROM catalogs WHERE name = ?1",
                Self::CATALOG_COLS
            ))?
            .query_row([name], Self::catalog_row)
            .optional()
    }

    /// Who may own catalog `name`: `personal` and only `personal` has no
    /// org (the table's CHECK, said in words), and an existing catalog never
    /// changes owner — that is a remove and an add. Read-only, so a caller
    /// with side effects of its own (`catalogs::add_catalog`'s clone) can
    /// refuse before taking them (PF18); [`Self::upsert_catalog`] runs it
    /// again as its own backstop. Whether a NEW catalog's org exists is
    /// `upsert_catalog`'s check.
    /// [`Self::check_catalog_owner`]'s refusal of a non-personal catalog
    /// with no org — shared so `fleet-hub catalog add` can tell exactly that
    /// refusal apart and add how to pass the org (`--org`).
    pub fn catalog_needs_an_org_message(name: &str) -> String {
        format!("catalog {name} needs an org: only `personal` belongs to none")
    }

    pub fn check_catalog_owner(
        &self,
        name: &str,
        org_id: Option<i64>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        match (name == "personal", org_id) {
            (true, Some(_)) => Err(IpcError::new(
                codes::E_INVALID,
                "the personal catalog belongs to no org",
            )),
            (false, None) => Err(IpcError::new(
                codes::E_INVALID,
                Self::catalog_needs_an_org_message(name),
            )),
            _ => match self.get_catalog_by_name(name)? {
                Some(existing) if existing.org_id != org_id => Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "catalog {name} belongs to another org; remove it and add it again to move it"
                    ),
                )),
                _ => Ok(()),
            },
        }
    }

    /// Add a catalog, or re-point an existing one (`repo_path`, `remote_url`;
    /// the load record is cleared, as `set_catalog_config` does). The owner
    /// rules are [`Self::check_catalog_owner`]'s.
    pub fn upsert_catalog(
        &self,
        name: &str,
        repo_path: &str,
        remote_url: Option<&str>,
        org_id: Option<i64>,
    ) -> Result<CatalogRow, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        self.check_catalog_owner(name, org_id)?;
        match org_id {
            None => {
                self.set_catalog_config(repo_path, remote_url)?;
            }
            Some(org) => match self.get_catalog_by_name(name)? {
                Some(existing) => {
                    self.conn.execute(
                        "UPDATE catalogs SET repo_path = ?2, remote_url = ?3, \
                         head_commit = NULL, last_loaded_at = NULL WHERE id = ?1",
                        rusqlite::params![existing.id, repo_path, remote_url],
                    )?;
                }
                None => {
                    if self.get_org(org)?.is_none() {
                        return Err(IpcError::new(
                            codes::E_NOTFOUND,
                            format!("org {org} not found"),
                        ));
                    }
                    self.conn.execute(
                        "INSERT INTO catalogs (name, repo_path, remote_url, org_id, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![name, repo_path, remote_url, org, now_unix()],
                    )?;
                }
            },
        }
        self.get_catalog_by_name(name)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("catalog {name} vanished")))
    }

    /// Remove an org catalog's row (Rulings R13). Its `host_layers` rows,
    /// admissions and grants go with it (`ON DELETE CASCADE`); inventory rows
    /// keep their place with `catalog_id` NULL (`ON DELETE SET NULL`).
    /// Open changeset cards naming it are withdrawn and their items let go of
    /// it (Assets M4).
    pub fn remove_catalog(&self, name: &str) -> Result<CatalogRemoval, crate::ipc_error::IpcError> {
        use crate::ipc_error::{codes, IpcError};
        let row = self
            .get_catalog_by_name(name)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no catalog named {name}")))?;
        if row.org_id.is_none() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "the personal catalog cannot be removed; point it elsewhere with `catalog set`",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let count = |sql: &str| -> rusqlite::Result<usize> {
            tx.query_row(sql, [row.id], |r| r.get::<_, i64>(0))
                .map(|n| n as usize)
        };
        let removal = CatalogRemoval {
            id: row.id,
            name: row.name.clone(),
            layer_rows: count("SELECT COUNT(*) FROM host_layers WHERE catalog_id = ?1")?,
            admissions: count("SELECT COUNT(*) FROM host_catalogs WHERE catalog_id = ?1")?,
            grants: count("SELECT COUNT(*) FROM client_catalog_grants WHERE catalog_id = ?1")?,
            cards: count(
                "SELECT COUNT(DISTINCT c.id) FROM changesets c \
                 JOIN changeset_items i ON i.changeset_id = c.id \
                 WHERE i.catalog_id = ?1 AND c.state IN ('proposed', 'failed')",
            )?,
        };
        // Assets M4 (R26): `changeset_items.catalog_id` has no ON DELETE
        // (the spec's DDL), so the cards that name this catalog let go of it
        // first: open ones are withdrawn, applied ones keep their history.
        tx.execute(
            "UPDATE changesets SET state = 'dismissed', error = ?2 \
             WHERE state IN ('proposed', 'failed') \
               AND id IN (SELECT changeset_id FROM changeset_items WHERE catalog_id = ?1)",
            rusqlite::params![
                row.id,
                format!("withdrawn: catalog {} was removed", row.name)
            ],
        )?;
        tx.execute(
            "UPDATE changeset_items SET catalog_id = NULL WHERE catalog_id = ?1",
            [row.id],
        )?;
        tx.execute("DELETE FROM catalogs WHERE id = ?1", [row.id])?;
        tx.commit()?;
        Ok(removal)
    }

    /// Admit `catalog_id` on a host (migration 093). Whether the host may use
    /// an admission (no org, an org catalog) is the service's check.
    pub fn admit_host_catalog(
        &self,
        host_alias: &str,
        catalog_id: i64,
    ) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO host_catalogs (host_alias, catalog_id, admitted_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![host_alias, catalog_id, now_unix()],
        )? > 0)
    }

    pub fn unadmit_host_catalog(
        &self,
        host_alias: &str,
        catalog_id: i64,
    ) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "DELETE FROM host_catalogs WHERE host_alias = ?1 AND catalog_id = ?2",
            rusqlite::params![host_alias, catalog_id],
        )? > 0)
    }

    /// The catalog ids `host_alias` admits, ascending.
    pub fn host_admissions(&self, host_alias: &str) -> Result<Vec<i64>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT catalog_id FROM host_catalogs WHERE host_alias = ?1 ORDER BY catalog_id",
            )?
            .query_map([host_alias], |r| r.get(0))?
            .collect()
    }

    /// The hosts that admit `catalog_id`, by alias.
    pub fn catalog_admissions(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT host_alias FROM host_catalogs WHERE catalog_id = ?1 ORDER BY host_alias",
            )?
            .query_map([catalog_id], |r| r.get(0))?
            .collect()
    }

    pub fn set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalogs SET head_commit=?1, last_loaded_at=?2 WHERE org_id IS NULL",
            rusqlite::params![head, loaded_at],
        )?;
        Ok(())
    }

    /// Set the HEAD/last-loaded-at of one catalog by id.
    pub fn set_catalog_head_for(
        &self,
        id: i64,
        head: &str,
        loaded_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalogs SET head_commit=?1, last_loaded_at=?2 WHERE id=?3",
            rusqlite::params![head, loaded_at, id],
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
            // `catalog_id` is looked up through a subquery rather than bound
            // directly: an id that names no row in `catalogs` (stale —
            // e.g. the catalog it pointed to was unloaded or dropped
            // between scans) resolves to `NULL` instead of failing the
            // foreign key, which would otherwise roll back and lose this
            // host's ENTIRE inventory over one row's stale reference.
            tx.execute(
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at, managed, secret_like, fleet_owned, catalog_id, drift_side)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, (SELECT id FROM catalogs WHERE id = ?12), ?13)",
                rusqlite::params![
                    host_alias, harness, r.kind, r.name, r.state, r.catalog_hash, r.host_hash, r.scanned_at,
                    if r.managed { 1 } else { 0 },
                    if r.secret_like { 1 } else { 0 },
                    if r.fleet_owned { 1 } else { 0 },
                    r.catalog_id,
                    r.drift_side,
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
            "SELECT host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at, managed, secret_like, fleet_owned, catalog_id, drift_side
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
                secret_like: row.get::<_, i64>(9)? != 0,
                fleet_owned: row.get::<_, i64>(10)? != 0,
                catalog_id: row.get(11)?,
                drift_side: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// The newest `scanned_at` per host, for hosts with any inventory row.
    pub fn inventory_last_scans(
        &self,
    ) -> Result<std::collections::BTreeMap<String, i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, MAX(scanned_at) FROM asset_inventory GROUP BY host_alias",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
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

    /// The id of the newest sync run that SB6 did not make — one whose
    /// summary is not marked `"auto": true` (Assets M4, final review I3).
    /// A row whose JSON does not parse counts as a person's.
    pub fn last_person_sync_run_id(&self) -> Result<Option<i64>, rusqlite::Error> {
        self.conn
            .prepare_cached(
                "SELECT id FROM sync_runs \
                 WHERE NOT (json_valid(summary_json) \
                            AND json_extract(summary_json, '$.auto') IS 1) \
                 ORDER BY id DESC LIMIT 1",
            )?
            .query_row([], |row| row.get(0))
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
    fn set_catalog_config_writes_the_personal_catalog_row() {
        let s = Store::open_in_memory().expect("open");
        assert!(s.personal_catalog().unwrap().is_none());
        s.set_catalog_config("/tmp/assets", Some("git@x:y.git"))
            .unwrap();
        let p = s.personal_catalog().unwrap().expect("personal row");
        assert_eq!(p.name, "personal");
        assert_eq!(p.org_id, None);
        assert_eq!(p.repo_path, "/tmp/assets");
        assert_eq!(p.remote_url.as_deref(), Some("git@x:y.git"));
        assert_eq!(s.list_catalogs().unwrap(), vec![p.clone()]);
        assert_eq!(s.get_catalog(p.id).unwrap(), Some(p));
    }

    #[test]
    fn set_catalog_head_for_targets_one_catalog() {
        let s = Store::open_in_memory().expect("open");
        s.set_catalog_config("/tmp/assets", None).unwrap();
        let id = s.personal_catalog().unwrap().unwrap().id;
        s.set_catalog_head_for(id, "abc", 7).unwrap();
        let cfg = s.get_catalog_config().unwrap().unwrap();
        assert_eq!(cfg.head_commit.as_deref(), Some("abc"));
        assert_eq!(cfg.last_loaded_at, Some(7));
    }

    #[test]
    fn only_the_personal_catalog_may_have_no_org() {
        let s = Store::open_in_memory().expect("open");
        let err = s
            .conn
            .execute(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('other', '/x', NULL, 0)",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"), "{err}");
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
            secret_like: false,
            fleet_owned: false,
            catalog_id: None,
            drift_side: None,
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

    /// `catalog_id` (Assets M2) is a real foreign key into `catalogs(id)`,
    /// but a stale or hand-built id that names no row there must not fail
    /// the whole insert — a scan's row for ONE asset must never roll back
    /// every other row the same scan computed for that host.
    #[test]
    fn replace_host_inventory_stores_null_for_an_unknown_catalog_id() {
        let s = Store::open_in_memory().expect("open");
        let row = AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "a".into(),
            state: "in_sync".into(),
            catalog_hash: Some("c".into()),
            host_hash: Some("h".into()),
            scanned_at: 1,
            managed: true,
            secret_like: false,
            fleet_owned: false,
            // No `catalogs` row has this id — not even the personal one,
            // since nothing was configured.
            catalog_id: Some(999_999),
            drift_side: None,
        };
        s.replace_host_inventory("local", "claude", &[row]).unwrap();
        let rows = s.list_inventory().unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].catalog_id, None);
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

    /// Assets M5 (R4): which side moved survives the store, and a row that
    /// says nothing reads back as nothing.
    #[test]
    fn inventory_round_trips_drift_side() {
        let s = Store::open_in_memory().unwrap();
        let row = |name: &str, side: Option<&str>| AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: "drifted".into(),
            managed: true,
            drift_side: side.map(String::from),
            ..Default::default()
        };
        s.replace_host_inventory(
            "local",
            "claude",
            &[
                row("a", Some("catalog")),
                row("b", Some("host")),
                row("c", None),
            ],
        )
        .unwrap();
        let got: Vec<Option<String>> = s
            .list_inventory()
            .unwrap()
            .into_iter()
            .map(|r| r.drift_side)
            .collect();
        assert_eq!(
            got,
            [Some("catalog".to_string()), Some("host".to_string()), None]
        );
    }

    #[test]
    fn inventory_round_trips_flags_and_reports_last_scans() {
        let s = Store::open_in_memory().unwrap();
        let row = |host: &str, name: &str, at: i64| AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: "unmanaged".into(),
            catalog_hash: None,
            host_hash: Some("h".into()),
            scanned_at: at,
            managed: false,
            secret_like: true,
            fleet_owned: true,
            catalog_id: None,
            drift_side: None,
        };
        s.replace_host_inventory(
            "local",
            "claude",
            &[row("local", "a", 5), row("local", "b", 9)],
        )
        .unwrap();
        s.replace_host_inventory("oci", "claude", &[row("oci", "a", 7)])
            .unwrap();
        let got = s.list_inventory().unwrap();
        assert!(got.iter().all(|r| r.secret_like && r.fleet_owned));
        let last = s.inventory_last_scans().unwrap();
        assert_eq!(last.get("local"), Some(&9));
        assert_eq!(last.get("oci"), Some(&7));
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

    /// `personal`, host `h`, org `acme` and its catalog. Returns
    /// `(store, org_id, acme_catalog_id)`.
    fn store_with_acme() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        s.upsert_host("h").unwrap();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(org.id)).unwrap();
        (s, org.id, acme.id)
    }

    #[test]
    fn upsert_catalog_adds_repoints_and_never_moves_a_catalog_between_owners() {
        use crate::ipc_error::codes::{E_INVALID, E_NOTFOUND};
        let (s, org, acme) = store_with_acme();
        s.set_catalog_head_for(acme, "abc", 5).unwrap();
        let again = s
            .upsert_catalog("acme", "/b", Some("git@x:y.git"), Some(org))
            .unwrap();
        assert_eq!(
            (
                again.id,
                again.repo_path.as_str(),
                again.remote_url.as_deref()
            ),
            (acme, "/b", Some("git@x:y.git"))
        );
        assert_eq!(
            (again.head_commit, again.last_loaded_at),
            (None, None),
            "a re-point clears the load record, as set_catalog_config does"
        );
        let other = s.add_org("other", None, false).unwrap();
        assert_eq!(
            s.upsert_catalog("acme", "/b", None, Some(other.id))
                .unwrap_err()
                .code,
            E_INVALID,
            "moving a catalog between owners is a remove and an add"
        );
        assert_eq!(
            s.upsert_catalog("beta", "/c", None, None).unwrap_err().code,
            E_INVALID,
            "only personal has no org"
        );
        assert_eq!(
            s.upsert_catalog("personal", "/c", None, Some(org))
                .unwrap_err()
                .code,
            E_INVALID
        );
        assert_eq!(
            s.upsert_catalog("beta", "/c", None, Some(9999))
                .unwrap_err()
                .code,
            E_NOTFOUND
        );
        let p = s.upsert_catalog("personal", "/p2", None, None).unwrap();
        assert_eq!(p.repo_path, "/p2");
        assert_eq!(s.get_catalog_by_name("personal").unwrap().unwrap().id, p.id);
    }

    #[test]
    fn admissions_are_per_host_and_catalog_and_go_with_the_host() {
        let (s, _org, acme) = store_with_acme();
        assert!(s.admit_host_catalog("h", acme).unwrap());
        assert!(
            !s.admit_host_catalog("h", acme).unwrap(),
            "admitting twice is a no-op"
        );
        assert_eq!(s.host_admissions("h").unwrap(), vec![acme]);
        assert_eq!(s.catalog_admissions(acme).unwrap(), vec!["h".to_string()]);
        assert!(s.unadmit_host_catalog("h", acme).unwrap());
        assert!(!s.unadmit_host_catalog("h", acme).unwrap());
        assert!(s.host_admissions("h").unwrap().is_empty());
        s.admit_host_catalog("h", acme).unwrap();
        s.delete_host("h").unwrap();
        assert!(s.catalog_admissions(acme).unwrap().is_empty());
    }

    /// Rulings R13: removing a catalog is config only — its layer
    /// assignments, admissions and grants go (CASCADE), inventory rows lose
    /// their `catalog_id` (SET NULL), `personal` can never be removed.
    #[test]
    fn removing_a_catalog_drops_its_assignments_admissions_and_grants_but_never_personal() {
        use crate::ipc_error::codes::{E_INVALID, E_NOTFOUND};
        let (s, _org, acme) = store_with_acme();
        s.set_host_layers_for("h", acme, Some("ops"), &["extra"])
            .unwrap();
        s.set_host_layers("h", Some("core"), &[]).unwrap();
        s.admit_host_catalog("h", acme).unwrap();
        s.insert_client_token("desk", "aa11", "full").unwrap();
        s.set_client_catalog_grant("desk", acme, true).unwrap();
        s.replace_host_inventory(
            "h",
            "claude",
            &[AssetInventoryRow {
                host_alias: "h".into(),
                harness: "claude".into(),
                kind: "skill".into(),
                name: "c".into(),
                state: "in_sync".into(),
                scanned_at: 1,
                managed: true,
                catalog_id: Some(acme),
                ..Default::default()
            }],
        )
        .unwrap();

        let gone = s.remove_catalog("acme").unwrap();
        assert_eq!(
            (
                gone.id,
                gone.name.as_str(),
                gone.layer_rows,
                gone.admissions,
                gone.grants
            ),
            (acme, "acme", 2, 1, 1)
        );
        assert!(s.get_catalog_by_name("acme").unwrap().is_none());
        assert_eq!(
            s.get_host_layers("h").unwrap().len(),
            1,
            "personal's assignment stays"
        );
        assert!(s.host_admissions("h").unwrap().is_empty());
        assert_eq!(s.list_inventory().unwrap()[0].catalog_id, None);
        assert_eq!(s.remove_catalog("personal").unwrap_err().code, E_INVALID);
        assert_eq!(s.remove_catalog("acme").unwrap_err().code, E_NOTFOUND);
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
