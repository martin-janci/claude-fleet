//! Client tokens: one row per paired client (a phone, a laptop browser).
//!
//! Unlike `host_tokens`, only the SHA-256 of the token is stored: the
//! plaintext is shown once at pairing and never needs to be displayed again.

use super::*;
use crate::ipc_error::codes;

impl Store {
    /// Insert a new client token row. `E_INVALID` when a *live* row already
    /// has this name (a revoked row does not block reuse — see the partial
    /// unique index on `client_tokens(name)`).
    pub fn insert_client_token(
        &self,
        name: &str,
        token_sha256: &str,
        mode: &str,
    ) -> Result<ClientTokenRow, crate::ipc_error::IpcError> {
        let at = now_unix();
        self.conn
            .execute(
                "INSERT INTO client_tokens (name, token_sha256, mode, created_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![name, token_sha256, mode, at],
            )
            .map_err(|e| {
                if is_unique_violation(&e) {
                    crate::ipc_error::IpcError::new(
                        codes::E_INVALID,
                        format!("a client named '{name}' already exists"),
                    )
                } else {
                    crate::ipc_error::IpcError::from(e)
                }
            })?;
        let id = self.conn.last_insert_rowid();
        get_client_token_by_id(&self.conn, id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_INTERNAL,
                format!("client token {id} vanished right after insert"),
            )
        })
    }

    /// Every client token row, newest first. `include_revoked` also returns
    /// revoked rows (kept for the audit trail); otherwise only live ones.
    pub fn list_client_tokens(
        &self,
        include_revoked: bool,
    ) -> Result<Vec<ClientTokenRow>, crate::ipc_error::IpcError> {
        let sql = if include_revoked {
            "SELECT id, name, token_sha256, mode, created_at, last_seen_at, revoked_at \
             FROM client_tokens ORDER BY id DESC"
        } else {
            "SELECT id, name, token_sha256, mode, created_at, last_seen_at, revoked_at \
             FROM client_tokens WHERE revoked_at IS NULL ORDER BY id DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], map_client_token_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every live (not revoked) client token row.
    pub fn active_client_tokens(&self) -> Result<Vec<ClientTokenRow>, crate::ipc_error::IpcError> {
        self.list_client_tokens(false)
    }

    /// Revoke the live token named `name`. `E_NOTFOUND` when there is none.
    pub fn revoke_client_token(
        &self,
        name: &str,
    ) -> Result<ClientTokenRow, crate::ipc_error::IpcError> {
        let at = now_unix();
        let n = self.conn.execute(
            "UPDATE client_tokens SET revoked_at = ?2 \
             WHERE name = ?1 AND revoked_at IS NULL",
            rusqlite::params![name, at],
        )?;
        if n == 0 {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                format!("no active client token named '{name}'"),
            ));
        }
        let id: i64 = self.conn.query_row(
            "SELECT id FROM client_tokens WHERE name = ?1 AND revoked_at = ?2",
            rusqlite::params![name, at],
            |row| row.get(0),
        )?;
        get_client_token_by_id(&self.conn, id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_INTERNAL,
                format!("client token {id} vanished right after revoke"),
            )
        })
    }

    /// Update `last_seen_at` to `now`, but only when the stored value is
    /// unset or more than 60 seconds stale — avoids a write on every request.
    pub fn touch_client_token(&self, id: i64, now: i64) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE client_tokens SET last_seen_at = ?2 \
             WHERE id = ?1 AND (last_seen_at IS NULL OR last_seen_at <= ?2 - 60)",
            rusqlite::params![id, now],
        )?;
        Ok(())
    }
}

/// Whether `e` is a SQLite UNIQUE constraint violation.
fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE,
            },
            _
        )
    )
}

fn map_client_token_row(row: &rusqlite::Row) -> rusqlite::Result<ClientTokenRow> {
    Ok(ClientTokenRow {
        id: row.get(0)?,
        name: row.get(1)?,
        token_sha256: row.get(2)?,
        mode: row.get(3)?,
        created_at: row.get(4)?,
        last_seen_at: row.get(5)?,
        revoked_at: row.get(6)?,
    })
}

fn get_client_token_by_id(
    conn: &rusqlite::Connection,
    id: i64,
) -> rusqlite::Result<Option<ClientTokenRow>> {
    conn.query_row(
        "SELECT id, name, token_sha256, mode, created_at, last_seen_at, revoked_at \
         FROM client_tokens WHERE id = ?1",
        rusqlite::params![id],
        map_client_token_row,
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().expect("store")
    }

    #[test]
    fn insert_list_and_resolve_by_hash() {
        let s = store();
        let row = s.insert_client_token("phone", "aa11", "full").unwrap();
        assert_eq!(row.name, "phone");
        assert_eq!(row.mode, "full");
        assert!(row.revoked_at.is_none());
        assert!(row.id > 0);
        let all = s.list_client_tokens(false).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].token_sha256, "aa11");
    }

    #[test]
    fn a_revoked_row_is_not_active_but_is_still_listed() {
        let s = store();
        s.insert_client_token("phone", "aa11", "full").unwrap();
        let revoked = s.revoke_client_token("phone").unwrap();
        assert!(revoked.revoked_at.is_some());
        assert!(s.active_client_tokens().unwrap().is_empty());
        assert_eq!(s.list_client_tokens(true).unwrap().len(), 1);
        assert!(s.list_client_tokens(false).unwrap().is_empty());
    }

    #[test]
    fn revoking_an_unknown_name_is_not_found() {
        let s = store();
        let e = s.revoke_client_token("nope").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
    }

    #[test]
    fn a_name_is_unique_and_a_revoked_name_can_be_reused() {
        let s = store();
        s.insert_client_token("phone", "aa11", "full").unwrap();
        let e = s.insert_client_token("phone", "bb22", "full").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        s.revoke_client_token("phone").unwrap();
        s.insert_client_token("phone", "bb22", "readonly").unwrap();
        let active = s.active_client_tokens().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].mode, "readonly");
    }

    #[test]
    fn touch_updates_last_seen_at_most_once_a_minute() {
        let s = store();
        let row = s.insert_client_token("phone", "aa11", "full").unwrap();
        s.touch_client_token(row.id, 1_000).unwrap();
        s.touch_client_token(row.id, 1_030).unwrap(); // inside the minute: ignored
        let seen = s.list_client_tokens(false).unwrap()[0].last_seen_at;
        assert_eq!(seen, Some(1_000));
        s.touch_client_token(row.id, 1_100).unwrap(); // past the minute: applied
        assert_eq!(
            s.list_client_tokens(false).unwrap()[0].last_seen_at,
            Some(1_100)
        );
    }
}
