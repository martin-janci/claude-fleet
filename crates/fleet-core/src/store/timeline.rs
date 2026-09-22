//! The per-session event timeline and the inter-session message inbox.

use super::*;

/// Whether a timeline write also announces itself on the event bus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Announce {
    Yes,
    No,
}

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
        self.insert_session_event_for(session_id, None, kind, detail)
    }

    /// [`Self::insert_session_event`] tagged with the conversation it belongs
    /// to. Emits `session:event` with the inserted row.
    pub fn insert_session_event_for(
        &self,
        session_id: i64,
        claude_session_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.write_session_event(session_id, claude_session_id, kind, detail, Announce::Yes)
    }

    /// [`Self::insert_session_event_for`] that writes the row and stays quiet.
    ///
    /// For an event whose only audience is the timeline when somebody next
    /// opens it. The audit row for a *read* is the case this exists for: on a
    /// fleet with a desktop and a phone attached, the desktop's 5 s
    /// conversation poll alone produced ~720 `session:event` frames an hour,
    /// each one fanned out to every connected client — 253 B apiece to a
    /// phone, to say that somebody else had just read something. Worse, a
    /// read-only paired client learned from them what the operator was doing.
    ///
    /// The row is still written, so the timeline and `session_history` are
    /// unchanged: this drops the live announcement, not the audit.
    pub fn insert_session_event_quietly(
        &self,
        session_id: i64,
        claude_session_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.write_session_event(session_id, claude_session_id, kind, detail, Announce::No)
    }

    fn write_session_event(
        &self,
        session_id: i64,
        claude_session_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
        announce: Announce,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = now_unix();
        self.conn.execute(
            "INSERT INTO session_events (session_id, at, kind, detail, claude_session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, at, kind, detail, claude_session_id],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM session_events WHERE session_id=?1 AND id NOT IN (\
                   SELECT id FROM session_events WHERE session_id=?1 \
                   ORDER BY at DESC, id DESC LIMIT ?2)",
            rusqlite::params![session_id, SESSION_EVENTS_CAP],
        )?;
        if announce == Announce::Yes {
            self.bus.session_event_added(&SessionEvent {
                id,
                session_id,
                at,
                kind: kind.to_string(),
                detail: detail.map(String::from),
                claude_session_id: claude_session_id.map(String::from),
            });
        }
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
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, kind, detail, claude_session_id FROM session_events \
                 WHERE session_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit], |row| {
            Ok(SessionEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                at: row.get(2)?,
                kind: row.get(3)?,
                detail: row.get(4)?,
                claude_session_id: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// This conversation's timeline events, oldest first (at most `limit`,
    /// the newest ones). Events recorded before migration 037 carry no
    /// conversation id and are not returned.
    pub fn list_conversation_events(
        &self,
        session_id: i64,
        claude_session_id: &str,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, kind, detail, claude_session_id FROM (\
                 SELECT * FROM session_events WHERE session_id = ?1 AND claude_session_id = ?2 \
                 ORDER BY at DESC, id DESC LIMIT ?3) ORDER BY at ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![session_id, claude_session_id, limit],
            |row| {
                Ok(SessionEvent {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    at: row.get(2)?,
                    kind: row.get(3)?,
                    detail: row.get(4)?,
                    claude_session_id: row.get(5)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        let at = now_unix();
        self.conn.execute(
            "INSERT INTO session_messages \
                   (from_session_id, to_session_id, body, kind, sent_at, reply_to) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![from_session_id, to_session_id, body, kind, at, reply_to],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One message by id (any recipient). Used to validate `reply_to`.
    pub fn get_message(
        &self,
        id: i64,
    ) -> Result<Option<SessionMessage>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM session_messages WHERE id = ?1"),
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
        let unread = if unread_only {
            " AND read_at IS NULL"
        } else {
            ""
        };
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM session_messages \
             WHERE to_session_id = ?1{unread} \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![to_session_id, limit], map_message_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every event of kind `opened` that no LATER event of kind `closed` on
    /// the same session has resolved, oldest first: `(session_id, event_id,
    /// detail)`. Generic over the two kinds — the store stays ignorant of
    /// what "opened"/"closed" mean to a caller (e.g. a move's wait-for-idle).
    pub fn unresolved_events(
        &self,
        opened: &str,
        closed: &str,
    ) -> Result<Vec<(i64, i64, Option<String>)>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT w.session_id, w.id, w.detail \
               FROM session_events w \
              WHERE w.kind = ?1 \
                AND NOT EXISTS (SELECT 1 FROM session_events c \
                                 WHERE c.session_id = w.session_id \
                                   AND c.kind = ?2 \
                                   AND c.id > w.id) \
              ORDER BY w.id",
        )?;
        let rows = stmt.query_map(rusqlite::params![opened, closed], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        let at = now_unix();
        let sql = format!(
            "UPDATE session_messages SET read_at = ?1 \
             WHERE to_session_id = ?2 AND read_at IS NULL AND id IN ({phs})",
            phs = in_clause(ids.len())
        );
        let params = params_then(rusqlite::params![at, recipient], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_events_are_the_opened_ones_no_later_close_resolved() {
        let s = Store::open_in_memory().unwrap();
        // Events take bare session ids, as the file's other tests use them.
        let (a, b) = (7_i64, 99_i64);
        s.insert_session_event(a, "open", Some("a1")).unwrap(); // resolved below
        s.insert_session_event(a, "close", None).unwrap();
        s.insert_session_event(a, "open", Some("a2")).unwrap(); // NOT resolved
        s.insert_session_event(b, "open", Some("b1")).unwrap(); // NOT resolved
        s.insert_session_event(b, "other", None).unwrap(); // not a close
        let got: Vec<String> = s
            .unresolved_events("open", "close")
            .unwrap()
            .into_iter()
            .map(|(_, _, d)| d.unwrap())
            .collect();
        assert_eq!(got, vec!["a2".to_string(), "b1".to_string()]);
    }

    #[test]
    fn a_close_on_another_session_resolves_nothing() {
        let s = Store::open_in_memory().unwrap();
        let (a, b) = (7_i64, 99_i64);
        s.insert_session_event(a, "open", Some("a1")).unwrap();
        s.insert_session_event(b, "close", None).unwrap();
        assert_eq!(s.unresolved_events("open", "close").unwrap().len(), 1);
    }

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

    /// A read is audited, and says nothing on the bus. The row is what the
    /// timeline is for; the frame was 253 B to every connected client telling
    /// it somebody else had just read something.
    #[test]
    fn a_quiet_timeline_write_is_stored_but_not_announced() {
        let (s, bus) = test_support::store_with_recorder();
        let sid = 7;

        s.insert_session_event(sid, "mcp_call", Some("send_prompt by controller"))
            .unwrap();
        s.insert_session_event_quietly(
            sid,
            None,
            "mcp_call",
            Some("list_sessions by client:phone"),
        )
        .unwrap();

        assert_eq!(
            bus.names(),
            vec!["session:event"],
            "the write announced itself; the read did not"
        );
        let rows = s.list_session_events(sid, 10).unwrap();
        assert_eq!(rows.len(), 2, "both are on the timeline either way");
        assert!(
            rows.iter()
                .any(|r| r.detail.as_deref() == Some("list_sessions by client:phone")),
            "the quiet one is still audited: {rows:?}"
        );
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
    fn list_conversation_events_returns_only_the_requested_conversation_oldest_first() {
        let s = Store::open_in_memory().expect("open");
        s.insert_session_event_for(7, Some("conv-a"), "prompt_sent", Some("1"))
            .unwrap();
        s.insert_session_event_for(7, Some("conv-b"), "prompt_sent", Some("2"))
            .unwrap();
        s.insert_session_event_for(7, Some("conv-a"), "turn_ended", Some("3"))
            .unwrap();
        // Pre-migration-034 event: no conversation id, never returned.
        s.insert_session_event(7, "status_change", Some("no-conv"))
            .unwrap();

        let a = s.list_conversation_events(7, "conv-a", 50).unwrap();
        assert_eq!(a.len(), 2, "only conv-a's events, none from conv-b or NULL");
        assert_eq!(a[0].detail.as_deref(), Some("1"), "oldest first");
        assert_eq!(a[1].detail.as_deref(), Some("3"));
        assert!(a
            .iter()
            .all(|e| e.claude_session_id.as_deref() == Some("conv-a")));

        // limit keeps the newest rows even though the result is oldest-first.
        s.insert_session_event_for(7, Some("conv-a"), "turn_ended", Some("4"))
            .unwrap();
        let limited = s.list_conversation_events(7, "conv-a", 2).unwrap();
        assert_eq!(
            limited.iter().map(|e| e.detail.clone()).collect::<Vec<_>>(),
            vec![Some("3".to_string()), Some("4".to_string())]
        );
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
