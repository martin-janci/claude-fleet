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
    ///
    /// `kind` is free text (no DB-level enum), but every caller in the
    /// codebase draws from one vocabulary, grouped by the subsystem that
    /// writes it:
    /// - reconcile (`service::sessions::reconcile`): `status_change`, `stuck`,
    ///   `lost` (detail is the `lost_reason`, e.g. `host_reboot`).
    /// - lifecycle (`service::sessions::lifecycle` / `prompt`):  `killed`,
    ///   `recreated`, `prompt_sent`.
    /// - host-reboot restore (`service::sessions::restore`, Task 3):
    ///   `session_restored`, `session_restore_failed` (detail is the error
    ///   message).
    /// - workspace repair (`service::repair::{EVENT_REPAIRED,
    ///   EVENT_REPAIR_FAILED}`): `workspace_repaired`, `workspace_repair_failed`.
    /// - move_session (`service::move_session::{EVENT_MOVED,
    ///   EVENT_MOVE_PARTIAL}`): `session_moved`, `session_move_partial`.
    /// - tasks (`service::tasks`): `task_dispatched`, `task_started`,
    ///   `task_done`, `task_failed`, `task_cancelled`.
    /// - inter-session messages (`service::messages`): `message_sent`,
    ///   `message_received`.
    /// - safe kill (`service::safe_kill`): `safe_kill_requested`,
    ///   `safe_kill_send_failed`, `safe_kill_failed`, `safe_kill_ready`,
    ///   `safe_kill_discarded`.
    /// - GC sweeper (`service::gc`): `gc_killed`, `gc_failed`.
    /// - tidy-up (`service::work::tidy`): `gc_tidied`, and `tidy_kept` (detail
    ///   is the unix second a person's keep holds until, work graph M11.3).
    /// - hooks (`service::hooks`): `notification`.
    /// - playbooks (`Store::record_playbook_applied` et al.): `playbook_applied`.
    /// - MCP call audit (`mcp::tools::support`): `mcp_call`.
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

    /// Events for `session_id` strictly after `after_id`, OLDEST first by
    /// id. The cursor path: paging oldest-first and advancing only to the
    /// last id returned is what makes a `limit` unable to skip a row. By id,
    /// never `at` — `at` is wall-clock and a clock step would reorder it.
    pub fn session_events_after(
        &self,
        session_id: i64,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, kind, detail, claude_session_id FROM session_events \
             WHERE session_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, after_id, limit], |row| {
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

    pub fn max_session_event_id(
        &self,
        session_id: i64,
    ) -> Result<Option<i64>, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT MAX(id) FROM session_events WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?)
    }

    /// The id of the latest conversation boundary for `session_id`. A
    /// transcript cursor stores it: `turn_seq` keeps counting across a
    /// /clear while the transcript FILE changes, so a moved generation means
    /// the old conversation's unread tail is not in the file being read.
    pub fn conversation_generation(
        &self,
        session_id: i64,
    ) -> Result<Option<i64>, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT MAX(id) FROM session_events WHERE session_id = ?1 \
             AND kind IN ('conversation_started','conversation_ended','compact_done')",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?)
    }

    /// Inbox messages for `session_id` strictly after `after_id`, OLDEST
    /// first by id. Resolved through the participant exactly as
    /// [`Self::list_inbox`] is, for the same reason (a moved session's mail
    /// follows the participant, not a raw `to_session_id`).
    pub fn inbox_after(
        &self,
        session_id: i64,
        after_id: i64,
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
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1){unread} \
             AND id > ?2 ORDER BY id ASC LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![session_id, after_id, limit],
            map_message_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn max_inbox_id(&self, session_id: i64) -> Result<Option<i64>, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT MAX(id) FROM session_messages \
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1)",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?)
    }

    /// The newest event of `session_id` whose kind is one of `kinds`.
    pub fn newest_session_event_of(
        &self,
        session_id: i64,
        kinds: &[&str],
    ) -> Result<Option<SessionEvent>, crate::ipc_error::IpcError> {
        let list = serde_json::to_string(kinds).unwrap_or_else(|_| "[]".into());
        Ok(self
            .conn
            .query_row(
                "SELECT id, session_id, at, kind, detail, claude_session_id FROM session_events \
                 WHERE session_id = ?1 AND kind IN (SELECT value FROM json_each(?2)) \
                 ORDER BY at DESC, id DESC LIMIT 1",
                rusqlite::params![session_id, list],
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
            )
            .optional()?)
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
    ///
    /// Both ends must name an existing `sessions` row (`E_NOTFOUND`
    /// otherwise). This is the chokepoint, not a courtesy check the callers
    /// duplicate: `ensure_participant_for_session` below mints an identity
    /// for whatever id it is handed, nothing ever retires one whose session
    /// does not exist, so the 7-day sweep never reaches it and its sender is
    /// never told. And because `sessions.id` is REUSED, the next session to
    /// take that id resolves the same participant and has the dead
    /// recipient's mail injected into its prompt context by
    /// `list_undelivered_for_session` — not merely made visible via `inbox`.
    /// `service::messages::send_message` checks both ends itself;
    /// `service::tasks::complete_task` (best-effort, `let _ =`) did not.
    pub fn insert_message(
        &self,
        from_session_id: i64,
        to_session_id: i64,
        body: &str,
        kind: &str,
        reply_to: Option<i64>,
    ) -> Result<i64, crate::ipc_error::IpcError> {
        let at = now_unix();
        for (label, id) in [("from", from_session_id), ("to", to_session_id)] {
            let exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                rusqlite::params![id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(crate::ipc_error::IpcError::new(
                    crate::ipc_error::codes::E_NOTFOUND,
                    format!("{label} session {id} not found"),
                ));
            }
        }
        let from_p = self.ensure_participant_for_session(from_session_id)?;
        let to_p = self.ensure_participant_for_session(to_session_id)?;
        self.conn.execute(
            "INSERT INTO session_messages \
               (from_session_id, to_session_id, from_participant_id, to_participant_id, \
                body, kind, sent_at, reply_to) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                from_session_id,
                to_session_id,
                from_p,
                to_p,
                body,
                kind,
                at,
                reply_to
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // `insert_message` is also called inside `Store::atomically`, where a
        // rollback must announce nothing. `notify_waiters()` only wakes
        // waiters, which then re-read the table and find nothing — a
        // spurious wake, never a false message, so signalling here
        // unconditionally (rather than deferring it until commit) is safe.
        self.message_notify.notify_waiters();
        Ok(id)
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

    /// True when `participant_id` is either end of message `id`.
    ///
    /// The durable way to ask "did this endpoint take part in that thread?":
    /// `from_session_id` / `to_session_id` name the `sessions` row as it was
    /// when the message was sent, and a move replaces that row, so a
    /// participant comparison is the only one that survives one. False for a
    /// message that does not exist, so a caller that already checked
    /// existence keeps its own `E_NOTFOUND` wording.
    pub fn message_involves_participant(
        &self,
        id: i64,
        participant_id: i64,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_messages \
             WHERE id = ?1 AND (from_participant_id = ?2 OR to_participant_id = ?2))",
            rusqlite::params![id, participant_id],
            |r| r.get(0),
        )?)
    }

    /// Newest-first messages addressed to `to_session_id`, capped at `limit`.
    /// When `unread_only`, only rows whose `read_at IS NULL` are returned.
    ///
    /// Resolved through the participant, not `to_session_id` (fix round 1,
    /// Important 2): `sessions.id` has no AUTOINCREMENT, so a killed
    /// session's numeric id can be reused by a later, unrelated session — a
    /// raw `to_session_id` match would then hand that new session the dead
    /// one's mail (and let it mark that mail read, silencing the real
    /// sender's `message_undeliverable`). Matching `list_undelivered_for_session`
    /// also means a moved session can read mail carried across the move, not
    /// just have it delivered to a hook.
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
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1){unread} \
             ORDER BY sent_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![to_session_id, limit], map_message_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The newest inbox message for `session_id` with `id > after_id`, or
    /// `None`. Purely id-ordered — `id`, unlike `sent_at`, is monotonic by
    /// construction (`INTEGER PRIMARY KEY`), so a waiter that only needs to
    /// know "is there anything newer than the last one I saw" can answer
    /// that without depending on the wall clock: a clock regression (an NTP
    /// step, a VM resume) can shift `sent_at` without touching `id`, and a
    /// `sent_at`-ordered scan could then hide a genuinely newer row for the
    /// rest of a wait's timeout. Used by [`crate::service::messages::wait_for_reply`].
    ///
    /// Resolved through the participant, not `session_id` (fix round 1,
    /// Important 2 — see [`Self::list_inbox`]'s doc comment for why a raw
    /// session id is unsafe here too: a fresh `wait_for_reply` call passes
    /// `after_id = 0`, which would otherwise match every message ever
    /// addressed to a reused numeric id).
    pub fn newest_inbox_message_after(
        &self,
        session_id: i64,
        after_id: i64,
    ) -> Result<Option<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM session_messages \
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1) \
               AND id > ?2 \
             ORDER BY id DESC LIMIT 1"
        );
        self.conn
            .query_row(
                &sql,
                rusqlite::params![session_id, after_id],
                map_message_row,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
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

    /// Mark a set of inbox messages as read. Only rows whose recipient
    /// PARTICIPANT resolves from `recipient`'s current session id are
    /// updated — never mark someone else's mail. Returns the number of rows
    /// that flipped from unread to read.
    ///
    /// Resolved through the participant, not `to_session_id` (fix round 1,
    /// Important 2 — see [`Self::list_inbox`]'s doc comment): a raw
    /// `to_session_id` match would let a session that reused a dead
    /// session's numeric id mark the dead session's mail read, silencing
    /// the real sender's `message_undeliverable`; and would refuse a moved
    /// session's attempt to mark its own carried mail read (the column
    /// still names the pre-move row), producing a false
    /// `message_undeliverable` on the sender 7 days later for mail that was
    /// in fact delivered and read.
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
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?2) \
               AND read_at IS NULL AND id IN ({phs})",
            phs = in_clause(ids.len())
        );
        let params = params_then(rusqlite::params![at, recipient], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }

    /// Messages waiting to be handed to `session_id`'s next hook response,
    /// OLDEST first — delivery replays conversation order, where the inbox
    /// shows the newest first.
    ///
    /// Resolved through the participant, not `to_session_id`, so a message
    /// addressed before a `move_session` still reaches the moved session.
    pub fn list_undelivered_for_session(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<SessionMessage>, crate::ipc_error::IpcError> {
        let sql = format!(
            "SELECT {MESSAGE_COLUMNS} FROM session_messages \
             WHERE to_participant_id = (SELECT id FROM participants WHERE session_id = ?1) \
               AND delivered_at IS NULL \
             ORDER BY sent_at ASC, id ASC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit], map_message_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Stamp `delivered_at` on rows that do not have it yet. "Handed to a hook
    /// response", never "the model read it" — there is no ack. Returns how
    /// many rows flipped, so a second call reports 0.
    pub fn mark_messages_delivered(
        &self,
        ids: &[i64],
    ) -> Result<usize, crate::ipc_error::IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let at = now_unix();
        let sql = format!(
            "UPDATE session_messages SET delivered_at = ?1 \
             WHERE delivered_at IS NULL AND id IN ({phs})",
            phs = in_clause(ids.len())
        );
        let params = params_then(rusqlite::params![at], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// Final review, Important 2: a participant must never exist for a
    /// session that does not. `ensure_participant_for_session` mints one for
    /// any id it is handed, nothing retires it, and `sessions.id` is REUSED —
    /// so a later session taking that id inherits the dead requester's mail
    /// and has it injected into its prompt context. `insert_message` is the
    /// one chokepoint every sender goes through (`service::tasks::complete_task`
    /// does not check existence itself), so it validates here.
    #[test]
    fn insert_message_refuses_a_session_that_does_not_exist() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let gone = 4242;
        let err = s
            .insert_message(a, gone, "hi", "message", None)
            .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND, "{err:?}");
        let err = s
            .insert_message(gone, a, "hi", "message", None)
            .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND, "{err:?}");
        assert!(
            s.participant_for_session(gone).unwrap().is_none(),
            "a refused insert must not have minted an identity for a dead session"
        );
        assert!(
            s.list_inbox(gone, false, 10).unwrap().is_empty(),
            "and nothing may be addressed to it"
        );
    }

    #[test]
    fn undelivered_is_oldest_first_and_excludes_delivered_rows() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m1 = s.insert_message(a, b, "one", "message", None).unwrap();
        let m2 = s.insert_message(a, b, "two", "message", None).unwrap();

        let got = s.list_undelivered_for_session(b, 10).unwrap();
        assert_eq!(
            got.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m1, m2],
            "delivery order is oldest first, unlike the newest-first inbox"
        );

        assert_eq!(s.mark_messages_delivered(&[m1]).unwrap(), 1);
        let got = s.list_undelivered_for_session(b, 10).unwrap();
        assert_eq!(got.iter().map(|m| m.id).collect::<Vec<_>>(), vec![m2]);
    }

    #[test]
    fn marking_delivered_does_not_mark_read() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        s.mark_messages_delivered(&[m]).unwrap();
        let row = s.get_message(m).unwrap().unwrap();
        assert_eq!(row.read_at, None, "delivered is not read");
        assert_eq!(
            s.list_inbox(b, true, 10).unwrap().len(),
            1,
            "a delivered message is still unread in the inbox"
        );
    }

    #[test]
    fn mark_delivered_is_idempotent() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        s.ensure_participant_for_session(b).unwrap();
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        assert_eq!(s.mark_messages_delivered(&[m]).unwrap(), 1);
        assert_eq!(
            s.mark_messages_delivered(&[m]).unwrap(),
            0,
            "a second stamp changes nothing"
        );
    }

    #[test]
    fn insert_message_fills_the_participant_columns() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "hi", "message", None).unwrap();
        let (from_p, to_p): (Option<i64>, Option<i64>) = s
            .conn
            .query_row(
                "SELECT from_participant_id, to_participant_id FROM session_messages WHERE id=?1",
                rusqlite::params![m],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            from_p,
            Some(s.participant_for_session(a).unwrap().unwrap().id)
        );
        assert_eq!(
            to_p,
            Some(s.participant_for_session(b).unwrap().unwrap().id)
        );
    }

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
        // The ids 1/5/9 below are REAL session rows now: `insert_message`
        // validates that both ends exist (final review, Important 2), and
        // this test asserts store mechanics, not the absence of that check.
        // `sessions.id` starts at 1 and increments, so seeding nine rows
        // makes 1, 5 and 9 exactly the rows these ids name.
        for i in 1..=9 {
            seed(&s, &format!("s{i}"));
        }
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
        // Real rows for the same reason as
        // `session_messages_inbox_roundtrip_and_mark_read` above.
        for i in 1..=5 {
            seed(&s, &format!("s{i}"));
        }
        let m1 = s.insert_message(1, 5, "q", "message", None).unwrap();
        let m2 = s.insert_message(5, 1, "a", "reply", Some(m1)).unwrap();
        assert_eq!(s.get_message(m2).unwrap().unwrap().reply_to, Some(m1));
        assert_eq!(s.get_message(m1).unwrap().unwrap().reply_to, None);
        assert!(s.get_message(999).unwrap().is_none());
        assert_eq!(s.list_inbox(1, false, 10).unwrap()[0].reply_to, Some(m1));
    }

    /// Fix round 1, Important 2 (the reused-id half). Replaces the
    /// assertion Task 13's rewrites dropped without a replacement
    /// (`list_inbox(dead).is_empty()`, which used to hold only because the
    /// old code hard-deleted a killed session's mail outright). `sessions.id`
    /// has no AUTOINCREMENT, so a killed session's numeric id can be reused
    /// by a later, unrelated session; without resolving through the
    /// participant, that new session would see — and be able to mark read —
    /// the dead session's mail, silencing the real sender's notice.
    #[test]
    fn a_session_reusing_a_killed_ids_row_sees_no_mail() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s
            .insert_message(a, b, "for the old beta", "message", None)
            .unwrap();
        s.delete_session(b).unwrap();

        // `beta` was the highest-numbered row, so SQLite's ordinary
        // max(rowid)+1 allocation reuses its exact id for the next insert
        // with no explicit id — this assertion is the whole point of the
        // test, not incidental.
        let reused = seed(&s, "beta-reincarnated");
        assert_eq!(
            reused, b,
            "the new row must reuse the old numeric id for this test to mean anything"
        );

        assert!(
            s.list_inbox(reused, false, 10).unwrap().is_empty(),
            "a session reusing a killed id must not see the dead session's mail"
        );
        assert_eq!(
            s.mark_messages_read(&[m], reused).unwrap(),
            0,
            "and must not be able to mark it read either"
        );
        assert_eq!(
            s.get_message(m).unwrap().unwrap().read_at,
            None,
            "the original message is untouched"
        );
    }

    /// Fix round 1, Important 2 (the false-undeliverable half). A moved
    /// session must be able to read and mark read the mail carried across
    /// the move via the ORDINARY inbox API (`list_inbox`/
    /// `mark_messages_read`), not just have it handed to a hook via
    /// `list_undelivered_for_session`/`mark_messages_delivered` — otherwise
    /// the mail is stuck permanently unread from the moved session's own
    /// point of view, and a later kill + the GC retention sweep produces a
    /// false `message_undeliverable` on the sender for mail that was in
    /// fact delivered and read.
    #[test]
    fn a_moved_sessions_carried_mail_is_readable_and_marking_it_read_prevents_a_false_undeliverable_notice(
    ) {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let src = seed(&s, "worker");
        let dst = seed(&s, "worker-moved");
        let m = s
            .insert_message(a, src, "follow me", "message", None)
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;

        // What finalise does: re-point, then kill the source row.
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();

        // The moved session can see its carried mail through the ordinary
        // inbox, and mark it read.
        let inbox = s.list_inbox(dst, false, 10).unwrap();
        assert_eq!(inbox.iter().map(|x| x.id).collect::<Vec<_>>(), vec![m]);
        assert_eq!(
            s.mark_messages_read(&[m], dst).unwrap(),
            1,
            "the moved session must be able to mark its own carried mail read"
        );

        // Time passes: the moved session itself is later killed, and the
        // retention sweep runs well past the window.
        s.delete_session(dst).unwrap();
        s.conn
            .execute(
                "UPDATE participants SET retired_at = retired_at - ?1 WHERE retired_at IS NOT NULL",
                rusqlite::params![crate::store::RETIRED_RETENTION_SECS + 60],
            )
            .unwrap();
        s.sweep_retired_participants(now_unix(), crate::store::RETIRED_RETENTION_SECS)
            .unwrap();

        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            !ev.iter().any(|e| e.kind == "message_undeliverable"),
            "mail that was actually delivered and read must never produce a false undeliverable notice: {ev:?}"
        );
    }

    /// `insert_session_event` returns `Result<(), IpcError>`, not the
    /// inserted row's id (the brief's draft test assumed the latter), so
    /// these tests recover the ids with a direct id-ordered query instead.
    fn event_ids(s: &Store, session_id: i64) -> Vec<i64> {
        let mut stmt = s
            .conn
            .prepare("SELECT id FROM session_events WHERE session_id = ?1 ORDER BY id ASC")
            .unwrap();
        stmt.query_map(rusqlite::params![session_id], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn events_after_are_oldest_first_by_id_and_exclude_the_watermark() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        s.insert_session_event(a, "prompt_sent", Some("1")).unwrap();
        s.insert_session_event(a, "prompt_sent", Some("2")).unwrap();
        s.insert_session_event(a, "prompt_sent", Some("3")).unwrap();
        let ids = event_ids(&s, a);
        let (e1, e2, e3) = (ids[0], ids[1], ids[2]);
        let got: Vec<i64> = s
            .session_events_after(a, e1, 50)
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(
            got,
            vec![e2, e3],
            "oldest-first, strictly after the watermark"
        );
        assert_eq!(s.max_session_event_id(a).unwrap(), Some(e3));
    }

    /// The paging rule the cursor depends on: with a limit, the OLDEST rows
    /// after the watermark come back, so advancing to the largest id returned
    /// can never step over one that was not returned.
    #[test]
    fn events_after_with_a_limit_return_the_oldest_rows_not_the_newest() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        for i in 0..5 {
            s.insert_session_event(a, "prompt_sent", Some(&i.to_string()))
                .unwrap();
        }
        let ids = event_ids(&s, a);
        let got: Vec<i64> = s
            .session_events_after(a, 0, 2)
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(got, vec![ids[0], ids[1]]);
    }

    #[test]
    fn inbox_after_resolves_by_participant_and_pages_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m1 = s.insert_message(a, b, "one", "message", None).unwrap();
        let m2 = s.insert_message(a, b, "two", "message", None).unwrap();
        let m3 = s.insert_message(a, b, "three", "message", None).unwrap();
        let got: Vec<i64> = s
            .inbox_after(b, m1, false, 50)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(got, vec![m2, m3]);
        assert_eq!(s.max_inbox_id(b).unwrap(), Some(m3));
        assert!(
            s.inbox_after(a, 0, false, 50).unwrap().is_empty(),
            "the sender's inbox is empty"
        );
    }

    #[test]
    fn the_generation_moves_only_on_a_conversation_boundary() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        assert_eq!(s.conversation_generation(a).unwrap(), None);
        s.insert_session_event(a, "conversation_started", None)
            .unwrap();
        let g1 = *event_ids(&s, a).last().unwrap();
        s.insert_session_event(a, "prompt_sent", None).unwrap();
        s.insert_session_event(a, "turn_done", None).unwrap();
        assert_eq!(
            s.conversation_generation(a).unwrap(),
            Some(g1),
            "ordinary events do not move it"
        );
        s.insert_session_event(a, "compact_done", None).unwrap();
        let g2 = *event_ids(&s, a).last().unwrap();
        assert_eq!(
            s.conversation_generation(a).unwrap(),
            Some(g2),
            "a compaction does"
        );
    }
}
