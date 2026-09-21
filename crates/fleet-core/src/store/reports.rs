//! The `error_reports` table: what the hub keeps of every participant's
//! error-level events. Bounded on insert (row cap) and on the tick (age).

use super::*;
use fleet_proto::report::Report;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRow {
    pub id: i64,
    pub received_at: i64,
    pub at: i64,
    pub origin: String,
    pub level: String,
    pub component: String,
    pub code: Option<String>,
    pub message: String,
    pub context: Option<serde_json::Value>,
    pub truncated: bool,
}

fn default_limit() -> u32 {
    100
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReportFilter {
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub since: Option<i64>,
    pub origin: Option<String>,
    pub level: Option<String>,
}

impl Default for ReportFilter {
    fn default() -> Self {
        ReportFilter {
            limit: default_limit(),
            since: None,
            origin: None,
            level: None,
        }
    }
}

impl Store {
    /// Append a batch under `origin`. Best-effort caller contract: a failure
    /// is logged and never blocks the sender.
    pub fn insert_reports(
        &self,
        origin: &str,
        reports: &[Report],
        received_at: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for r in reports {
            let context = r
                .context
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .unwrap_or(None);
            tx.execute(
                "INSERT INTO error_reports \
                 (received_at, at, origin, level, component, code, message, context, truncated) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    received_at,
                    r.at,
                    origin,
                    r.level,
                    r.component,
                    r.code,
                    r.message,
                    context,
                    r.truncated as i64
                ],
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Keep the `max_rows` newest rows; returns how many were deleted.
    pub fn prune_reports_to(&self, max_rows: u64) -> Result<usize, crate::ipc_error::IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM error_reports WHERE id NOT IN (\
               SELECT id FROM error_reports ORDER BY received_at DESC, id DESC LIMIT ?1)",
            rusqlite::params![max_rows as i64],
        )?)
    }

    /// Delete rows received before `cutoff`; returns how many.
    pub fn sweep_reports_older_than(
        &self,
        cutoff: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM error_reports WHERE received_at < ?1",
            rusqlite::params![cutoff],
        )?)
    }

    /// Newest first, filtered.
    pub fn list_reports(
        &self,
        f: &ReportFilter,
    ) -> Result<Vec<ReportRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, received_at, at, origin, level, component, code, message, context, truncated \
             FROM error_reports \
             WHERE (?1 IS NULL OR received_at >= ?1) \
               AND (?2 IS NULL OR origin = ?2) \
               AND (?3 IS NULL OR level = ?3) \
             ORDER BY received_at DESC, id DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![f.since, f.origin, f.level, f.limit as i64],
            |row| {
                let context: Option<String> = row.get(8)?;
                Ok(ReportRow {
                    id: row.get(0)?,
                    received_at: row.get(1)?,
                    at: row.get(2)?,
                    origin: row.get(3)?,
                    level: row.get(4)?,
                    component: row.get(5)?,
                    code: row.get(6)?,
                    message: row.get(7)?,
                    context: context.and_then(|c| serde_json::from_str(&c).ok()),
                    truncated: row.get::<_, i64>(9)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn r(msg: &str) -> Report {
        Report::error("fleet_core::ssh", msg)
    }

    #[test]
    fn insert_then_list_newest_first_with_filters() {
        let s = store();
        let mut a = r("first");
        a.code = Some("E_SSH".into());
        a.context = Some(serde_json::json!({ "k": 1 }));
        assert_eq!(
            s.insert_reports("client:desk", &[a.clone()], 100).unwrap(),
            1
        );
        assert_eq!(
            s.insert_reports("host:box", &[r("second")], 200).unwrap(),
            1
        );
        let rows = s.list_reports(&ReportFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].message, "second");
        assert_eq!(rows[1].code.as_deref(), Some("E_SSH"));
        assert_eq!(rows[1].context, Some(serde_json::json!({ "k": 1 })));
        assert_eq!(rows[1].origin, "client:desk");
        let since = s
            .list_reports(&ReportFilter {
                since: Some(150),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(since.len(), 1);
        let origin = s
            .list_reports(&ReportFilter {
                origin: Some("client:desk".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(origin[0].message, "first");
        let limited = s
            .list_reports(&ReportFilter {
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn prune_keeps_the_newest_rows() {
        let s = store();
        for i in 0..10 {
            s.insert_reports("hub", &[r(&format!("m{i}"))], 1000 + i)
                .unwrap();
        }
        assert_eq!(s.prune_reports_to(3).unwrap(), 7);
        let rows = s.list_reports(&ReportFilter::default()).unwrap();
        assert_eq!(
            rows.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(),
            ["m9", "m8", "m7"]
        );
    }

    #[test]
    fn sweep_deletes_only_older_rows() {
        let s = store();
        s.insert_reports("hub", &[r("old")], 100).unwrap();
        s.insert_reports("hub", &[r("new")], 500).unwrap();
        assert_eq!(s.sweep_reports_older_than(300).unwrap(), 1);
        assert_eq!(
            s.list_reports(&ReportFilter::default()).unwrap()[0].message,
            "new"
        );
    }
}
