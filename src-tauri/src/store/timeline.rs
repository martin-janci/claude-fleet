//! The per-session event timeline and the inter-session message inbox.

use super::*;

impl Store {
    /// Append one row to the per-session event timeline (migration 013). The
    /// timeline is append-only; callers must treat a write failure as
    /// non-fatal (log + continue) so it can never block the mutation that
    /// produced the event.
    ///
    /// Each insert also prunes the session's timeline down to
    /// `SESSION_EVENTS_CAP` newest rows. A status flap observed by the
    /// background reconcile tick can otherwise grow one session's timeline
    /// without bound (observed: ~200k `status_change` rows per session); the
    /// prune is a cheap indexed subselect and keeps the table bounded.
    pub fn insert_session_event(
        &self,
        session_id: i64,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_events (session_id, at, kind, detail) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, at, kind, detail],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        self.conn
            .execute(
                "DELETE FROM session_events WHERE session_id=?1 AND id NOT IN (\
                   SELECT id FROM session_events WHERE session_id=?1 \
                   ORDER BY at DESC, id DESC LIMIT ?2)",
                rusqlite::params![session_id, SESSION_EVENTS_CAP],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(())
    }

    /// Return the newest-first event timeline for a session, capped at `limit`.
    /// Ordering is `at DESC, id DESC` so events inserted within the same second
    /// still come back in insertion order (newest first).
    pub fn list_session_events(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, session_id, at, kind, detail FROM session_events \
                 WHERE session_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, limit], |row| {
                Ok(SessionEvent {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    at: row.get(2)?,
                    kind: row.get(3)?,
                    detail: row.get(4)?,
                })
            })
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Insert one inter-session message (migration 015). `sent_at` is stamped
    /// here as the current unix epoch. Returns the new row id so the caller
    /// can include it in the pane-delivery header.
    pub fn insert_message(
        &self,
        from_session_id: i64,
        to_session_id: i64,
        body: &str,
        kind: &str,
        reply_to: Option<i64>,
    ) -> Result<i64, crate::ipc_error::IpcError> {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO session_messages \
                   (from_session_id, to_session_id, body, kind, sent_at, reply_to) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![from_session_id, to_session_id, body, kind, at, reply_to],
            )
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One message by id (any recipient). Used to validate `reply_to`.
    pub fn get_message(
        &self,
        id: i64,
    ) -> Result<Option<SessionMessage>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
                 FROM session_messages WHERE id = ?1",
                rusqlite::params![id],
                map_message_row,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    /// Newest-first messages addressed to `to_session_id`, capped at `limit`.
    /// When `unread_only`, only rows whose `read_at IS NULL` are returned.
    pub fn list_inbox(
        &self,
        to_session_id: i64,
        unread_only: bool,
        limit: i64,
    ) -> Result<Vec<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = if unread_only {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
             FROM session_messages \
             WHERE to_session_id = ?1 AND read_at IS NULL \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        } else {
            "SELECT id, from_session_id, to_session_id, body, kind, sent_at, read_at, reply_to \
             FROM session_messages \
             WHERE to_session_id = ?1 \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(crate::ipc_error::IpcError::from)?;
        let rows = stmt
            .query_map(rusqlite::params![to_session_id, limit], map_message_row)
            .map_err(crate::ipc_error::IpcError::from)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(crate::ipc_error::IpcError::from)?);
        }
        Ok(out)
    }

    /// Mark a set of inbox messages as read. Only rows whose `to_session_id`
    /// matches `recipient` are updated — never mark someone else's mail.
    /// Returns the number of rows that flipped from unread to read.
    pub fn mark_messages_read(
        &self,
        ids: &[i64],
        recipient: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE session_messages SET read_at = ?1 \
             WHERE to_session_id = ?2 AND read_at IS NULL AND id IN ({placeholders})",
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&at, &recipient];
        for id in ids {
            params.push(id);
        }
        let n = self
            .conn
            .execute(sql.as_str(), rusqlite::params_from_iter(params))
            .map_err(crate::ipc_error::IpcError::from)?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_events_insert_then_list_newest_first_with_limit() {
        let s = Store::open_in_memory().expect("open");
        // Insert several events for session 7 (and a decoy for another session).
        s.insert_session_event(7, "status_change", Some("working"))
            .unwrap();
        s.insert_session_event(7, "prompt_sent", Some("hello"))
            .unwrap();
        s.insert_session_event(7, "stuck", Some("auth_menu"))
            .unwrap();
        s.insert_session_event(99, "killed", None).unwrap();

        // Newest-first (at DESC, id DESC): same-second inserts come back in
        // reverse insertion order.
        let all = s.list_session_events(7, 50).unwrap();
        assert_eq!(all.len(), 3, "decoy session 99 must be excluded");
        assert_eq!(all[0].kind, "stuck");
        assert_eq!(all[0].detail.as_deref(), Some("auth_menu"));
        assert_eq!(all[1].kind, "prompt_sent");
        assert_eq!(all[2].kind, "status_change");
        assert_eq!(all[2].session_id, 7);

        // Limit caps the result to the newest N.
        let limited = s.list_session_events(7, 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].kind, "stuck");
        assert_eq!(limited[1].kind, "prompt_sent");

        // NULL detail round-trips.
        let other = s.list_session_events(99, 50).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].detail, None);
    }

    #[test]
    fn insert_session_event_caps_timeline_per_session() {
        let s = Store::open_in_memory().expect("open");
        // Insert well past the cap for session 7, plus a decoy for session 8.
        for i in 0..(SESSION_EVENTS_CAP + 25) {
            s.insert_session_event(7, "status_change", Some(&format!("v{i}")))
                .unwrap();
        }
        s.insert_session_event(8, "killed", None).unwrap();

        let count: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, SESSION_EVENTS_CAP, "timeline capped per session");
        // Newest rows survive, oldest are pruned.
        let newest = s.list_session_events(7, 1).unwrap();
        assert_eq!(
            newest[0].detail.as_deref(),
            Some(&*format!("v{}", SESSION_EVENTS_CAP + 24))
        );
        // The other session's timeline is untouched.
        assert_eq!(s.list_session_events(8, 50).unwrap().len(), 1);
    }

    #[test]
    fn session_messages_inbox_roundtrip_and_mark_read() {
        let s = Store::open_in_memory().expect("open");
        // Two messages to session 5, one decoy to session 9.
        let m1 = s.insert_message(1, 5, "hello", "message", None).unwrap();
        let m2 = s.insert_message(2, 5, "second", "task", Some(m1)).unwrap();
        s.insert_message(1, 9, "noise", "message", None).unwrap();

        // list_inbox returns newest-first and excludes the decoy.
        let all = s.list_inbox(5, false, 50).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, m2);
        assert_eq!(all[0].body, "second");
        assert_eq!(all[0].kind, "task");
        assert_eq!(all[0].reply_to, Some(m1));
        assert_eq!(all[1].reply_to, None);
        assert_eq!(all[1].id, m1);
        assert!(all.iter().all(|m| m.read_at.is_none()));

        // unread_only filter and limit.
        let unread = s.list_inbox(5, true, 1).unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, m2);

        // Mark only one as read; the other stays unread. A mismatched
        // recipient cannot mark someone else's mail.
        let updated = s.mark_messages_read(&[m1, m2], 5).unwrap();
        assert_eq!(updated, 2);
        let again = s.list_inbox(5, true, 50).unwrap();
        assert!(again.is_empty(), "all unread were marked");
        let foreign = s.mark_messages_read(&[m1], 9).unwrap();
        assert_eq!(foreign, 0, "wrong recipient cannot mark");
    }

    #[test]
    fn messages_carry_reply_to() {
        let s = Store::open_in_memory().unwrap();
        let m1 = s.insert_message(1, 5, "q", "message", None).unwrap();
        let m2 = s.insert_message(5, 1, "a", "reply", Some(m1)).unwrap();
        assert_eq!(s.get_message(m2).unwrap().unwrap().reply_to, Some(m1));
        assert_eq!(s.get_message(m1).unwrap().unwrap().reply_to, None);
        assert!(s.get_message(999).unwrap().is_none());
        assert_eq!(s.list_inbox(1, false, 10).unwrap()[0].reply_to, Some(m1));
    }
}
