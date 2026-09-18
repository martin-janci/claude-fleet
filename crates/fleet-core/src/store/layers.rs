//! Per-host layer assignment: one active role plus ordered contexts.

use crate::store::Store;
use serde::Serialize;

/// One row of `host_layers`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostLayerRow {
    pub host_alias: String,
    pub layer_name: String,
    /// `"role"` | `"context"`.
    pub axis: String,
    pub position: i64,
    pub active: bool,
}

fn row_from(r: &rusqlite::Row<'_>) -> Result<HostLayerRow, rusqlite::Error> {
    Ok(HostLayerRow {
        host_alias: r.get(0)?,
        layer_name: r.get(1)?,
        axis: r.get(2)?,
        position: r.get(3)?,
        active: r.get::<_, i64>(4)? != 0,
    })
}

const COLS: &str = "host_alias, layer_name, axis, position, active";

impl Store {
    pub fn get_host_layers(&self, host_alias: &str) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers WHERE host_alias=?1 AND active=1 \
                 ORDER BY axis, position, layer_name"
            ))?
            .query_map(rusqlite::params![host_alias], row_from)?
            .collect()
    }

    pub fn list_all_host_layers(&self) -> Result<Vec<HostLayerRow>, rusqlite::Error> {
        self.conn
            .prepare(&format!(
                "SELECT {COLS} FROM host_layers ORDER BY host_alias, axis, position, layer_name"
            ))?
            .query_map([], row_from)?
            .collect()
    }

    /// Replace a host's assignment wholesale: one optional role plus the
    /// contexts in application order. Runs in one transaction so a host is
    /// never left with a half-written assignment.
    pub fn set_host_layers(
        &self,
        host_alias: &str,
        role: Option<&str>,
        contexts: &[&str],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM host_layers WHERE host_alias=?1",
            rusqlite::params![host_alias],
        )?;
        if let Some(r) = role {
            tx.execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES (?1, ?2, 'role', 0, 1)",
                rusqlite::params![host_alias, r],
            )?;
        }
        for (i, c) in contexts.iter().enumerate() {
            tx.execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES (?1, ?2, 'context', ?3, 1)",
                rusqlite::params![host_alias, c, i as i64],
            )?;
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    /// `PRAGMA foreign_keys = ON` is enforced (`schema.rs:247`), and
    /// `host_layers.host_alias` references `hosts(alias)` — so the host row
    /// must exist or every insert trips the constraint.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s
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
        s.conn
            .execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES ('local', 'a', 'role', 0, 1)",
                [],
            )
            .unwrap();
        let err = s
            .conn
            .execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES ('local', 'b', 'role', 0, 1)",
                [],
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNIQUE constraint failed") || msg.contains("idx_host_active_role"),
            "expected a unique-constraint failure, got: {msg}"
        );
    }
}
