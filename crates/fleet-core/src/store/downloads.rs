//! File downloads (migration 095): a file a session sent from its host,
//! copied into this machine's data dir. The rules — who may send, who sees
//! a row, the byte budget and where the bytes live — are in
//! `service::downloads`; this is the rows.

use super::{now_unix, Store};
use crate::events::{EventBus as _, RowChange};
use rusqlite::{OptionalExtension, Result};

/// One download, as every client receives it (`list_downloads`, the design's
/// `Download`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DownloadRow {
    pub id: i64,
    pub at: i64,
    pub host_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The session's org when it was sent: what the scope check reads once
    /// the session row is gone. Never on the wire.
    #[serde(skip)]
    pub org_id: Option<i64>,
    /// Whose file it is: the session's owner when it was sent (migration
    /// 148), what the scope check reads once the session row is gone.
    /// Never on the wire.
    #[serde(skip)]
    pub owner_person_id: Option<i64>,
    pub path: String,
    pub name: String,
    pub size: i64,
    /// `fetching` | `ready` | `failed`
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// `agent` | `person`
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloaded_at: Option<i64>,
    /// When the GC sweep will drop it; derived from `downloads.keep_secs`
    /// by the service, never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Bytes copied so far while `fetching`; filled in by the service from
    /// the copy in flight, never stored. Absent from a hub that predates it,
    /// and then a client shows the copy without a count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_bytes: Option<i64>,
    /// A person paused the copy (gap plan G7.15, Toasts board "Pause"):
    /// filled in by the service while `fetching`, never stored. Absent when
    /// it runs, and from a hub that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
}

/// A download to insert, in state `fetching`.
pub struct NewDownload<'a> {
    pub host_alias: &'a str,
    pub session_id: Option<i64>,
    pub session_name: Option<&'a str>,
    pub org_id: Option<i64>,
    pub path: &'a str,
    pub name: &'a str,
    pub size: i64,
    pub source: &'a str,
    pub note: Option<&'a str>,
}

const COLS: &str = "id, at, host_alias, session_id, session_name, org_id, path, name, size, \
                    state, error, sha256, source, note, ready_at, downloaded_at, owner_person_id";

fn row(r: &rusqlite::Row<'_>) -> Result<DownloadRow> {
    Ok(DownloadRow {
        id: r.get(0)?,
        at: r.get(1)?,
        host_alias: r.get(2)?,
        session_id: r.get(3)?,
        session_name: r.get(4)?,
        org_id: r.get(5)?,
        path: r.get(6)?,
        name: r.get(7)?,
        size: r.get(8)?,
        state: r.get(9)?,
        error: r.get(10)?,
        sha256: r.get(11)?,
        source: r.get(12)?,
        note: r.get(13)?,
        ready_at: r.get(14)?,
        downloaded_at: r.get(15)?,
        owner_person_id: r.get(16)?,
        expires_at: None,
        fetched_bytes: None,
        paused: None,
    })
}

impl Store {
    fn download_changed(&self, id: i64) {
        self.bus.emit(&RowChange::DownloadChanged(id));
    }

    pub fn insert_download(&self, d: &NewDownload<'_>) -> Result<DownloadRow> {
        self.conn.execute(
            "INSERT INTO downloads (at, host_alias, session_id, session_name, org_id, path,
                                    name, size, source, note, owner_person_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     (SELECT owner_person_id FROM sessions WHERE id = ?3))",
            rusqlite::params![
                now_unix(),
                d.host_alias,
                d.session_id,
                d.session_name,
                d.org_id,
                d.path,
                d.name,
                d.size,
                d.source,
                d.note
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.download_changed(id);
        self.download(id).map(|r| r.expect("the row just inserted"))
    }

    pub fn download(&self, id: i64) -> Result<Option<DownloadRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM downloads WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
    }

    /// Every row, newest first.
    pub fn downloads(&self) -> Result<Vec<DownloadRow>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM downloads ORDER BY id DESC"))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    /// A `fetching` row's bytes are in place. `false` when it was not
    /// fetching (removed or failed meanwhile).
    pub fn finish_download(&self, id: i64, sha256: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE downloads SET state = 'ready', sha256 = ?2, ready_at = ?3
             WHERE id = ?1 AND state = 'fetching'",
            rusqlite::params![id, sha256, now_unix()],
        )?;
        if n == 1 {
            self.download_changed(id);
        }
        Ok(n == 1)
    }

    /// A `fetching` row copied another slice: the row did not change, but
    /// its `fetched_bytes` did, so clients re-read it.
    pub fn note_download_progress(&self, id: i64) {
        self.download_changed(id);
    }

    /// A `fetching` row failed with `error`.
    pub fn fail_download(&self, id: i64, error: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE downloads SET state = 'failed', error = ?2
             WHERE id = ?1 AND state = 'fetching'",
            rusqlite::params![id, error],
        )?;
        if n == 1 {
            self.download_changed(id);
        }
        Ok(n == 1)
    }

    /// Every `fetching` row fails: the process that was copying it is gone.
    /// Run once at startup, before anything starts a new copy.
    pub fn fail_interrupted_downloads(&self) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut st = self
                .conn
                .prepare("SELECT id FROM downloads WHERE state = 'fetching'")?;
            let ids = st.query_map([], |r| r.get(0))?;
            ids.collect::<Result<_>>()?
        };
        for id in &ids {
            self.fail_download(*id, "interrupted: the copy stopped when fleet restarted")?;
        }
        Ok(ids.len())
    }

    pub fn mark_downloaded(&self, id: i64) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE downloads SET downloaded_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now_unix()],
        )?;
        if n == 1 {
            self.download_changed(id);
        }
        Ok(())
    }

    /// Drop the row. `false` when it was already gone.
    pub fn delete_download(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        if n == 1 {
            self.download_changed(id);
        }
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(name: &str) -> NewDownload<'_> {
        NewDownload {
            host_alias: "web-1",
            session_id: Some(3),
            session_name: Some("fleet-a"),
            org_id: None,
            path: "/tmp/x",
            name,
            size: 5,
            source: "agent",
            note: None,
        }
    }

    #[test]
    fn a_download_moves_from_fetching_once() {
        let s = Store::open_in_memory().unwrap();
        let a = s.insert_download(&new("a")).unwrap();
        assert_eq!(a.state, "fetching");
        assert!(s.finish_download(a.id, "abc").unwrap());
        assert!(!s.fail_download(a.id, "late").unwrap());
        let a = s.download(a.id).unwrap().unwrap();
        assert_eq!(
            (a.state.as_str(), a.sha256.as_deref()),
            ("ready", Some("abc"))
        );
        assert!(a.ready_at.is_some());

        let b = s.insert_download(&new("b")).unwrap();
        assert_eq!(s.fail_interrupted_downloads().unwrap(), 1);
        assert_eq!(s.download(b.id).unwrap().unwrap().state, "failed");

        let names: Vec<_> = s.downloads().unwrap().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["b", "a"]);
        assert!(s.delete_download(a.id).unwrap());
        assert!(!s.delete_download(a.id).unwrap());
    }
}
