//! Per-host layer assignment: one active role plus ordered contexts, per
//! catalog (Assets M2: `host_layers`'s primary key is now `(host_alias,
//! catalog_id, layer_name)`, one active role per `(host_alias, catalog_id)`).

use crate::store::Store;
use serde::{Deserialize, Serialize};

/// One row of `host_layers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostLayerRow {
    pub host_alias: String,
    /// Which catalog this assignment belongs to (migration 091). Wire field:
    /// `#[serde(default)]` so an older hub's `catalog_list_layers` /
    /// `catalog_set_host_layers` answer (no `catalog_id` yet) still parses.
    #[serde(default)]
    pub catalog_id: i64,
    pub layer_name: String,
    /// `"role"` | `"context"`.
    pub axis: String,
    pub position: i64,
    pub active: bool,
}

fn row_from(r: &rusqlite::Row<'_>) -> Result<HostLayerRow, rusqlite::Error> {
    Ok(HostLayerRow {
        host_alias: r.get(0)?,
        catalog_id: r.get(1)?,
        layer_name: r.get(2)?,
        axis: r.get(3)?,
        position: r.get(4)?,
        active: r.get::<_, i64>(5)? != 0,
    })
}

const COLS: &str = "host_alias, catalog_id, layer_name, axis, position, active";

impl Store {
    /// Every catalog's active rows for `host_alias`, ordered by catalog then
    /// axis/position/name.
    pub fn get_host_layers(&self, host_alias: &str) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers WHERE host_alias=?1 AND active=1 \
                 ORDER BY catalog_id, axis, position, layer_name"
            ))?
            .query_map(rusqlite::params![host_alias], row_from)?
            .collect()
    }

    /// `host_alias`'s active rows in one catalog.
    pub fn get_host_layers_for(
        &self,
        host_alias: &str,
        catalog_id: i64,
    ) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers WHERE host_alias=?1 AND catalog_id=?2 AND active=1 \
                 ORDER BY axis, position, layer_name"
            ))?
            .query_map(rusqlite::params![host_alias, catalog_id], row_from)?
            .collect()
    }

    pub fn list_all_host_layers(&self) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers ORDER BY host_alias, catalog_id, axis, position, layer_name"
            ))?
            .query_map([], row_from)?
            .collect()
    }

    /// Replace a host's assignment wholesale in the personal catalog: one
    /// optional role plus the contexts in application order. Errors with
    /// `QueryReturnedNoRows` if no personal catalog is configured yet.
    pub fn set_host_layers(
        &self,
        host_alias: &str,
        role: Option<&str>,
        contexts: &[&str],
    ) -> Result<(), rusqlite::Error> {
        let catalog_id = self
            .personal_catalog()?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?
            .id;
        self.set_host_layers_for(host_alias, catalog_id, role, contexts)
    }

    /// Replace a host's assignment wholesale in one catalog: one optional
    /// role plus the contexts in application order. Runs in one transaction
    /// so a host is never left with a half-written assignment. Another
    /// catalog's rows for the same host are untouched.
    pub fn set_host_layers_for(
        &self,
        host_alias: &str,
        catalog_id: i64,
        role: Option<&str>,
        contexts: &[&str],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM host_layers WHERE host_alias=?1 AND catalog_id=?2",
            rusqlite::params![host_alias, catalog_id],
        )?;
        if let Some(r) = role {
            tx.execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES (?1, ?2, ?3, 'role', 0, 1)",
                rusqlite::params![host_alias, catalog_id, r],
            )?;
        }
        for (i, c) in contexts.iter().enumerate() {
            tx.execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES (?1, ?2, ?3, 'context', ?4, 1)",
                rusqlite::params![host_alias, catalog_id, c, i as i64],
            )?;
        }
        tx.commit()
    }

    /// Put one catalog's `host_layers` back exactly as `rows` had them
    /// (Assets M4: undo and a failed apply, Rulings R12/R20). Rows of other
    /// catalogs in `rows` are ignored; a row whose host has been deleted
    /// since is skipped. Answers how many rows were written.
    ///
    /// It REPLACES the catalog's rows: every host's assignments in that
    /// catalog become the snapshot's, so a layer change made in that catalog
    /// after the snapshot (by hand, or for a host the snapshot does not name)
    /// is undone too.
    pub fn restore_host_layers(
        &self,
        catalog_id: i64,
        rows: &[HostLayerRow],
    ) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let n = restore_rows(&tx, catalog_id, rows)?;
        tx.commit()?;
        Ok(n)
    }
}

/// [`Store::restore_host_layers`] on `conn`, inside the caller's
/// transaction (an undo records it with the card's state, Assets M4).
pub(super) fn restore_rows(
    conn: &rusqlite::Connection,
    catalog_id: i64,
    rows: &[HostLayerRow],
) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "DELETE FROM host_layers WHERE catalog_id = ?1",
        [catalog_id],
    )?;
    let mut n = 0;
    let mut insert = conn.prepare(
        "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM hosts WHERE alias = ?1)",
    )?;
    for r in rows.iter().filter(|r| r.catalog_id == catalog_id) {
        n += insert.execute(rusqlite::params![
            r.host_alias,
            catalog_id,
            r.layer_name,
            r.axis,
            r.position,
            r.active as i64
        ])?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    /// `PRAGMA foreign_keys = ON` is enforced (`schema.rs:247`), and
    /// `host_layers.host_alias` references `hosts(alias)` — so the host row
    /// must exist or every insert trips the constraint. `set_host_layers`
    /// now also needs a personal catalog to target.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s.set_catalog_config("/p", None).expect("personal catalog");
        s
    }

    /// Assets M4 (undo, failed apply): one catalog's rows go back exactly as
    /// they were; a row whose host was deleted since is skipped.
    #[test]
    fn restore_host_layers_replaces_one_catalog_and_skips_hosts_gone_since() {
        let s = store_with_local();
        s.upsert_host("oci").unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        s.set_host_layers("local", Some("core"), &["a"]).unwrap();
        s.set_host_layers("oci", None, &["b"]).unwrap();
        let before = s.list_all_host_layers().unwrap();
        s.set_host_layers("local", None, &["changed"]).unwrap();
        s.set_host_layers("oci", None, &[]).unwrap();
        s.delete_host("oci").unwrap();
        assert_eq!(
            s.restore_host_layers(p, &before).unwrap(),
            2,
            "oci's row is skipped"
        );
        let local: Vec<_> = before
            .into_iter()
            .filter(|r| r.host_alias == "local")
            .collect();
        assert_eq!(s.list_all_host_layers().unwrap(), local);
    }

    #[test]
    fn set_and_get_round_trip_with_context_order() {
        let s = store_with_local();
        s.set_host_layers("local", Some("workstation"), &["papayapos", "writing"])
            .unwrap();
        let rows = s.get_host_layers("local").unwrap();
        let role: Vec<_> = rows.iter().filter(|r| r.axis == "role").collect();
        assert_eq!(role.len(), 1);
        assert_eq!(role[0].layer_name, "workstation");
        let mut ctx: Vec<_> = rows.iter().filter(|r| r.axis == "context").collect();
        ctx.sort_by_key(|r| r.position);
        assert_eq!(
            ctx.iter()
                .map(|r| r.layer_name.as_str())
                .collect::<Vec<_>>(),
            vec!["papayapos", "writing"]
        );
    }

    #[test]
    fn set_replaces_the_previous_assignment_entirely() {
        let s = store_with_local();
        s.set_host_layers("local", Some("a"), &["x", "y"]).unwrap();
        s.set_host_layers("local", Some("b"), &["z"]).unwrap();
        let rows = s.get_host_layers("local").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.layer_name == "b" && r.axis == "role"));
        assert!(rows
            .iter()
            .any(|r| r.layer_name == "z" && r.axis == "context"));
    }

    #[test]
    fn a_host_with_no_assignment_returns_nothing() {
        let s = store_with_local();
        assert!(s.get_host_layers("local").unwrap().is_empty());
    }

    #[test]
    fn clearing_the_role_is_allowed() {
        let s = store_with_local();
        s.set_host_layers("local", Some("a"), &[]).unwrap();
        s.set_host_layers("local", None, &["x"]).unwrap();
        let rows = s.get_host_layers("local").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].axis, "context");
    }

    /// `idx_host_active_role` is a partial unique index, not an application
    /// check — confirm it actually fires against a raw insert that bypasses
    /// `set_host_layers` (which itself can never violate it, since it always
    /// deletes the host's rows before inserting the new role).
    #[test]
    fn schema_index_rejects_a_second_active_role_for_one_host() {
        let s = store_with_local();
        let catalog_id = s.personal_catalog().unwrap().unwrap().id;
        s.conn
            .execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES ('local', ?1, 'a', 'role', 0, 1)",
                [catalog_id],
            )
            .unwrap();
        let err = s
            .conn
            .execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES ('local', ?1, 'b', 'role', 0, 1)",
                [catalog_id],
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNIQUE constraint failed") || msg.contains("idx_host_active_role"),
            "expected a unique-constraint failure, got: {msg}"
        );
    }

    #[test]
    fn layers_are_kept_per_catalog() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        s.conn
            .execute(
                "INSERT INTO orgs (name, created_at) VALUES ('acme', 0);
             ",
                [],
            )
            .unwrap();
        let org: i64 = s
            .conn
            .query_row("SELECT id FROM orgs WHERE name='acme'", [], |r| r.get(0))
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('acme', '/a', ?1, 0)",
                [org],
            )
            .unwrap();
        let acme: i64 = s
            .conn
            .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                r.get(0)
            })
            .unwrap();

        s.set_host_layers("h", Some("core"), &[]).unwrap(); // personal
        s.set_host_layers_for("h", acme, Some("ops"), &["extra"])
            .unwrap(); // same host, other catalog

        let p = s.get_host_layers_for("h", personal).unwrap();
        assert_eq!(
            p.iter().map(|r| r.layer_name.as_str()).collect::<Vec<_>>(),
            vec!["core"]
        );
        let a = s.get_host_layers_for("h", acme).unwrap();
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|r| r.catalog_id == acme));
        assert_eq!(s.get_host_layers("h").unwrap().len(), 3);
        // Re-setting one catalog's layers leaves the other's alone.
        s.set_host_layers_for("h", acme, None, &[]).unwrap();
        assert_eq!(s.get_host_layers("h").unwrap().len(), 1);
    }
}
