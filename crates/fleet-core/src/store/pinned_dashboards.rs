//! Dashboards a person pinned (migration 178, persona design step 6): one
//! row per dashboard id and person. Who may read and write a row and what
//! it may hold are in `service::dashboards`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// One pinned dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedDashboardRow {
    /// The block's `id`.
    pub id: String,
    /// Whose it is; `None` = fleet's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    pub title: String,
    /// The `dashboard` fleet.ui/1 block.
    pub spec: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The device it was pinned from; `None` = the hub's own desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub pinned_at: i64,
    pub updated_at: i64,
    /// Whether the version before the last replacement is kept, so
    /// `dashboards { undo }` can put it back (migration 220).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_undo: bool,
}

/// What one pin writes.
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedDashboardWrite<'a> {
    pub id: &'a str,
    pub person_id: Option<i64>,
    pub title: &'a str,
    pub spec: &'a serde_json::Value,
    pub why: Option<&'a str>,
    pub device: Option<&'a str>,
}

const COLS: &str = "dash_id, person_id, title, spec, why, device, pinned_at, updated_at, \
                    previous_spec IS NOT NULL";

fn row(r: &rusqlite::Row<'_>) -> Result<PinnedDashboardRow> {
    let spec: String = r.get(3)?;
    Ok(PinnedDashboardRow {
        id: r.get(0)?,
        person_id: r.get(1)?,
        title: r.get(2)?,
        // A spec this build cannot read stays a row; the service drops it.
        spec: serde_json::from_str(&spec).unwrap_or(serde_json::Value::Null),
        why: r.get(4)?,
        device: r.get(5)?,
        pinned_at: r.get(6)?,
        updated_at: r.get(7)?,
        can_undo: r.get(8)?,
    })
}

impl Store {
    /// Pin a dashboard, or replace the one of the same id and person (its
    /// `pinned_at` stays). A replacement that changes the spec keeps the
    /// version it overwrote, for [`Store::undo_dashboard`].
    pub fn pin_dashboard(&self, w: &PinnedDashboardWrite<'_>) -> Result<PinnedDashboardRow> {
        self.conn.execute(
            "INSERT INTO pinned_dashboards
               (dash_id, person_id, title, spec, why, device, pinned_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT (dash_id, COALESCE(person_id, 0)) DO UPDATE SET
               previous_spec = CASE WHEN pinned_dashboards.spec = excluded.spec
                 THEN pinned_dashboards.previous_spec ELSE pinned_dashboards.spec END,
               previous_device = CASE WHEN pinned_dashboards.spec = excluded.spec
                 THEN pinned_dashboards.previous_device ELSE pinned_dashboards.device END,
               title = excluded.title, spec = excluded.spec, why = excluded.why,
               device = CASE WHEN pinned_dashboards.spec = excluded.spec
                 THEN pinned_dashboards.device ELSE excluded.device END,
               updated_at = excluded.updated_at",
            rusqlite::params![
                w.id,
                w.person_id,
                w.title,
                w.spec.to_string(),
                w.why,
                w.device,
                now_unix()
            ],
        )?;
        Ok(self
            .pinned_dashboard(w.id, w.person_id)?
            .expect("the row just written"))
    }

    pub fn pinned_dashboard(
        &self,
        id: &str,
        person_id: Option<i64>,
    ) -> Result<Option<PinnedDashboardRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM pinned_dashboards
                     WHERE dash_id = ?1 AND COALESCE(person_id, 0) = COALESCE(?2, 0)"
                ),
                rusqlite::params![id, person_id],
                row,
            )
            .optional()
    }

    /// Every pinned dashboard, in the order pinned.
    pub fn pinned_dashboards(&self) -> Result<Vec<PinnedDashboardRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM pinned_dashboards ORDER BY pinned_at, dash_id"
        ))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    /// Put back the version the last replacement overwrote, and forget it
    /// (one step). `None` when the row is gone or keeps no earlier version.
    pub fn undo_dashboard(
        &self,
        id: &str,
        person_id: Option<i64>,
    ) -> Result<Option<PinnedDashboardRow>> {
        let n = self.conn.execute(
            "UPDATE pinned_dashboards SET
               spec = previous_spec,
               title = COALESCE(json_extract(previous_spec, '$.title'), title),
               device = previous_device,
               previous_spec = NULL, previous_device = NULL,
               updated_at = ?3
             WHERE dash_id = ?1 AND COALESCE(person_id, 0) = COALESCE(?2, 0)
               AND previous_spec IS NOT NULL",
            rusqlite::params![id, person_id, now_unix()],
        )?;
        if n == 0 {
            return Ok(None);
        }
        self.pinned_dashboard(id, person_id)
    }

    /// `true` when it was pinned.
    pub fn unpin_dashboard(&self, id: &str, person_id: Option<i64>) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM pinned_dashboards
             WHERE dash_id = ?1 AND COALESCE(person_id, 0) = COALESCE(?2, 0)",
            rusqlite::params![id, person_id],
        )? > 0)
    }
}
