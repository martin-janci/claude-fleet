//! Dashboard feeds (migration 222): the values `feed` widgets show, one row
//! per owner and key. Who may write which and what a value may hold are in
//! `service::dashboards`; this is the rows.

use super::Store;
use rusqlite::Result;
use serde::{Deserialize, Serialize};

/// Who writes a feed: a person's device, Control's agent, a routine's run.
pub const DASHBOARD_FEED_WRITERS: &[&str] = &["person", "agent", "routine"];

/// One feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardFeedRow {
    pub key: String,
    /// Whose it is; `None` = fleet's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    pub value: String,
    /// person | agent | routine.
    pub by: String,
    /// The routine's name, for a routine's write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_ref: Option<String>,
    pub updated_at: i64,
}

const COLS: &str = "key, NULLIF(owner, 0), value, by, by_ref, updated_at";

fn row(r: &rusqlite::Row<'_>) -> Result<DashboardFeedRow> {
    Ok(DashboardFeedRow {
        key: r.get(0)?,
        person_id: r.get(1)?,
        value: r.get(2)?,
        by: r.get(3)?,
        by_ref: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

impl Store {
    /// Write feed `key` of `person_id` (`None` = fleet's own), replacing
    /// what it held.
    pub fn set_dashboard_feed(
        &self,
        person_id: Option<i64>,
        key: &str,
        value: &str,
        by: &str,
        by_ref: Option<&str>,
        now: i64,
    ) -> Result<DashboardFeedRow> {
        self.conn.execute(
            "INSERT INTO dashboard_feeds (owner, key, value, by, by_ref, updated_at)
             VALUES (COALESCE(?1, 0), ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (owner, key) DO UPDATE SET
               value = excluded.value, by = excluded.by, by_ref = excluded.by_ref,
               updated_at = excluded.updated_at",
            rusqlite::params![person_id, key, value, by, by_ref, now],
        )?;
        Ok(DashboardFeedRow {
            key: key.to_string(),
            person_id,
            value: value.to_string(),
            by: by.to_string(),
            by_ref: by_ref.map(str::to_string),
            updated_at: now,
        })
    }

    /// `true` when feed `key` of `person_id` was there.
    pub fn delete_dashboard_feed(&self, person_id: Option<i64>, key: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM dashboard_feeds WHERE owner = COALESCE(?1, 0) AND key = ?2",
            rusqlite::params![person_id, key],
        )? > 0)
    }

    /// The feeds of `person_id` (`None` = fleet's own), by key.
    pub fn dashboard_feeds(&self, person_id: Option<i64>) -> Result<Vec<DashboardFeedRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM dashboard_feeds WHERE owner = COALESCE(?1, 0) ORDER BY key"
        ))?;
        let rows = st.query_map([person_id], row)?;
        rows.collect()
    }

    /// Feed `key` of `person_id`, if it is kept.
    pub fn dashboard_feed(
        &self,
        person_id: Option<i64>,
        key: &str,
    ) -> Result<Option<DashboardFeedRow>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM dashboard_feeds WHERE owner = COALESCE(?1, 0) AND key = ?2"
                ),
                rusqlite::params![person_id, key],
                row,
            )
            .optional()
    }

    /// How many feeds `person_id` keeps besides `key`.
    pub fn count_other_dashboard_feeds(&self, person_id: Option<i64>, key: &str) -> Result<usize> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM dashboard_feeds WHERE owner = COALESCE(?1, 0) AND key <> ?2",
            rusqlite::params![person_id, key],
            |r| r.get::<_, i64>(0).map(|n| n as usize),
        )
    }

    /// Every feed, by owner and key.
    pub fn all_dashboard_feeds(&self) -> Result<Vec<DashboardFeedRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM dashboard_feeds ORDER BY owner, key"
        ))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feed_is_written_replaced_read_and_removed_per_owner() {
        let s = Store::open_in_memory().unwrap();
        s.set_dashboard_feed(None, "deploys", "3 today", "agent", None, 10)
            .unwrap();
        s.set_dashboard_feed(Some(7), "deploys", "1", "routine", Some("Nightly"), 11)
            .unwrap();
        let row = s
            .set_dashboard_feed(None, "deploys", "4 today", "person", None, 12)
            .unwrap();
        assert_eq!(row.value, "4 today");
        let mine = s.dashboard_feeds(None).unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].value, "4 today");
        assert_eq!(mine[0].by, "person");
        assert_eq!(mine[0].person_id, None);
        let theirs = s.dashboard_feeds(Some(7)).unwrap();
        assert_eq!(theirs[0].by_ref.as_deref(), Some("Nightly"));
        assert_eq!(theirs[0].person_id, Some(7));
        assert_eq!(s.all_dashboard_feeds().unwrap().len(), 2);
        s.set_dashboard_feed(None, "ci", "green", "agent", None, 13)
            .unwrap();
        assert_eq!(s.count_other_dashboard_feeds(None, "deploys").unwrap(), 1);
        assert_eq!(s.count_other_dashboard_feeds(None, "new").unwrap(), 2);
        assert_eq!(s.count_other_dashboard_feeds(Some(7), "x").unwrap(), 1);
        assert_eq!(
            s.dashboard_feed(None, "ci").unwrap().unwrap().updated_at,
            13
        );
        assert!(s.dashboard_feed(Some(7), "ci").unwrap().is_none());
        assert!(s.delete_dashboard_feed(None, "ci").unwrap());
        assert!(s.delete_dashboard_feed(None, "deploys").unwrap());
        assert!(!s.delete_dashboard_feed(None, "deploys").unwrap());
        assert_eq!(s.dashboard_feeds(Some(7)).unwrap().len(), 1);
    }
}
