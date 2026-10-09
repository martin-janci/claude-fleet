//! Control's Library (migration 141): the files a person put on a host, by
//! Upload or as a prompt's attachment. Who sees a row is
//! `service::library`'s; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

/// One placed file, as clients receive it (`library`'s `items`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LibraryItemRow {
    pub id: i64,
    pub at: i64,
    /// `upload` | `attachment`
    pub kind: String,
    pub host_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The session's org when the file was placed: what the scope check
    /// reads once the session row is gone. Never on the wire.
    #[serde(skip)]
    pub org_id: Option<i64>,
    /// Where the file is on the host.
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
}

/// A placed file to record.
pub struct NewLibraryItem<'a> {
    pub kind: &'a str,
    pub host_alias: &'a str,
    pub session_id: Option<i64>,
    pub session_name: Option<&'a str>,
    pub org_id: Option<i64>,
    pub path: &'a str,
    pub name: &'a str,
    pub size: Option<i64>,
}

/// How many rows the index keeps, newest first.
pub const KEEP: i64 = 5000;

const COLS: &str = "id, at, kind, host_alias, session_id, session_name, org_id, path, name, size";

fn row(r: &rusqlite::Row<'_>) -> Result<LibraryItemRow> {
    Ok(LibraryItemRow {
        id: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        host_alias: r.get(3)?,
        session_id: r.get(4)?,
        session_name: r.get(5)?,
        org_id: r.get(6)?,
        path: r.get(7)?,
        name: r.get(8)?,
        size: r.get(9)?,
    })
}

impl Store {
    pub fn insert_library_item(&self, d: &NewLibraryItem<'_>) -> Result<LibraryItemRow> {
        self.conn.execute(
            "INSERT INTO library_items (at, kind, host_alias, session_id, session_name, org_id,
                                        path, name, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                now_unix(),
                d.kind,
                d.host_alias,
                d.session_id,
                d.session_name,
                d.org_id,
                d.path,
                d.name,
                d.size
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // The index keeps the newest KEEP rows: every prompt's attachment
        // lands here, and an index nobody reads past its first page need
        // not grow without end.
        self.conn.execute(
            "DELETE FROM library_items WHERE id <= ?1 - ?2",
            rusqlite::params![id, KEEP],
        )?;
        self.library_item(id)
            .map(|r| r.expect("the row just inserted"))
    }

    pub fn library_item(&self, id: i64) -> Result<Option<LibraryItemRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM library_items WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
    }

    /// Every row, newest first.
    pub fn library_items(&self) -> Result<Vec<LibraryItemRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM library_items ORDER BY id DESC"
        ))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    /// Drop the row (the file stays on the host). `false` when it was
    /// already gone.
    pub fn delete_library_item(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM library_items WHERE id = ?1", [id])?;
        Ok(n == 1)
    }
}
