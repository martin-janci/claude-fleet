//! Fleet events (migration 201): what happened to the fleet rather than to
//! one session — a host gone or back, an account at its limit, a mission
//! finished, a ticket done or checked — for event routines. Who writes them and when is
//! `service::fleet_events`; this is the rows.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// A host fleet could not reach (it was reachable before).
pub const HOST_OFFLINE: &str = "host_offline";
/// A host reachable again.
pub const HOST_ONLINE: &str = "host_online";
/// A host low on disk or memory.
pub const HOST_LOW: &str = "host_low";
/// An account at its 5-hour or weekly limit.
pub const ACCOUNT_LIMITED: &str = "account_limited";
/// An account under its limit again.
pub const ACCOUNT_AVAILABLE: &str = "account_available";
/// A Claude login fleet found signed out.
pub const LOGIN_SIGNED_OUT: &str = "login_signed_out";
/// A mission completed.
pub const MISSION_DONE: &str = "mission_done";
/// A mission failed.
pub const MISSION_FAILED: &str = "mission_failed";

/// A person set a work item done (`service::work::ticket_check`).
pub const TICKET_DONE: &str = "ticket_done";
/// An AI check of a ticket's finished work passed.
pub const CHECK_PASSED: &str = "check_passed";
/// An AI check found a ticket's finished work not ready.
pub const CHECK_FAILED: &str = "check_failed";

/// Every fleet event kind, the order the editors offer them.
pub const FLEET_EVENTS: [&str; 11] = [
    HOST_OFFLINE,
    HOST_ONLINE,
    HOST_LOW,
    ACCOUNT_LIMITED,
    ACCOUNT_AVAILABLE,
    LOGIN_SIGNED_OUT,
    MISSION_DONE,
    MISSION_FAILED,
    TICKET_DONE,
    CHECK_PASSED,
    CHECK_FAILED,
];

/// Rows the table keeps; an insert drops the oldest past it.
pub const FLEET_EVENTS_CAP: i64 = 5000;

/// One fleet event to write.
#[derive(Debug, Clone, Default)]
pub struct NewFleetEvent<'a> {
    pub kind: &'a str,
    pub subject: &'a str,
    pub detail: Option<&'a str>,
    pub host_alias: Option<&'a str>,
    pub org_id: Option<i64>,
    pub person_id: Option<i64>,
}

/// One stored fleet event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetEventRow {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
}

const COLS: &str = "id, at, kind, subject, detail, host_alias, org_id, person_id";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FleetEventRow> {
    Ok(FleetEventRow {
        id: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        subject: r.get(3)?,
        detail: r.get(4)?,
        host_alias: r.get(5)?,
        org_id: r.get(6)?,
        person_id: r.get(7)?,
    })
}

impl Store {
    /// Write one fleet event and keep the newest [`FLEET_EVENTS_CAP`].
    pub fn insert_fleet_event(&self, e: &NewFleetEvent<'_>) -> Result<i64, IpcError> {
        self.insert_fleet_event_at(e, now_unix())
    }

    /// [`Self::insert_fleet_event`] at `at`.
    pub fn insert_fleet_event_at(&self, e: &NewFleetEvent<'_>, at: i64) -> Result<i64, IpcError> {
        self.conn.execute(
            "INSERT INTO fleet_events (at, kind, subject, detail, host_alias, org_id, person_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                at,
                e.kind,
                e.subject,
                e.detail,
                e.host_alias,
                e.org_id,
                e.person_id
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if id % 100 == 0 {
            self.conn.execute(
                "DELETE FROM fleet_events WHERE id <= ?1",
                [id - FLEET_EVENTS_CAP],
            )?;
        }
        Ok(id)
    }

    /// The newest fleet event's id (0 for none): where a new routine on a
    /// fleet event starts looking.
    pub fn latest_fleet_event_id(&self) -> Result<i64, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM fleet_events", [], |r| {
                r.get(0)
            })?)
    }

    /// Fleet events of `kind` after `cursor`, oldest first, at most `limit`.
    pub fn fleet_events_of_kind_after(
        &self,
        cursor: i64,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<FleetEventRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM fleet_events WHERE kind = ?2 AND id > ?1 ORDER BY id LIMIT ?3"
        ))?;
        let rows = stmt.query_map(rusqlite::params![cursor, kind, limit], row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The newest `limit` fleet events of `kind`, newest first.
    pub fn recent_fleet_events_of_kind(
        &self,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<FleetEventRow>, IpcError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM fleet_events WHERE kind = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![kind, limit], row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many fleet events of `kind` came after `cursor`.
    pub fn count_fleet_events_of_kind_after(
        &self,
        cursor: i64,
        kind: &str,
    ) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM fleet_events WHERE kind = ?2 AND id > ?1",
            rusqlite::params![cursor, kind],
            |r| r.get(0),
        )?)
    }

    /// The fleet event `id`.
    pub fn fleet_event_by_id(&self, id: i64) -> Result<Option<FleetEventRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM fleet_events WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev<'a>(kind: &'a str, subject: &'a str) -> NewFleetEvent<'a> {
        NewFleetEvent {
            kind,
            subject,
            ..Default::default()
        }
    }

    #[test]
    fn events_read_by_kind_past_a_cursor_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.latest_fleet_event_id().unwrap(), 0);
        let a = s
            .insert_fleet_event(&ev("host_offline", "host:mac"))
            .unwrap();
        s.insert_fleet_event(&ev("host_online", "host:mac"))
            .unwrap();
        let c = s
            .insert_fleet_event(&ev("host_offline", "host:nas"))
            .unwrap();
        let got = s.fleet_events_of_kind_after(0, "host_offline", 10).unwrap();
        assert_eq!(got.iter().map(|e| e.id).collect::<Vec<_>>(), vec![a, c]);
        let got = s.fleet_events_of_kind_after(a, "host_offline", 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].subject, "host:nas");
        assert_eq!(
            s.count_fleet_events_of_kind_after(0, "host_offline")
                .unwrap(),
            2
        );
        assert_eq!(s.latest_fleet_event_id().unwrap(), c);
        let recent = s.recent_fleet_events_of_kind("host_offline", 1).unwrap();
        assert_eq!(recent[0].id, c);
        assert_eq!(
            s.fleet_event_by_id(a).unwrap().unwrap().kind,
            "host_offline"
        );
    }

    #[test]
    fn the_table_keeps_the_newest_rows_only() {
        let s = Store::open_in_memory().unwrap();
        for _ in 0..(FLEET_EVENTS_CAP + 200) {
            s.insert_fleet_event(&ev("host_offline", "host:mac"))
                .unwrap();
        }
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM fleet_events", [], |r| r.get(0))
            .unwrap();
        assert!(n <= FLEET_EVENTS_CAP + 100, "{n} rows kept");
    }
}
