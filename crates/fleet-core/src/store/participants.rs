//! Durable identity for every addressable endpoint (migration 043).
//!
//! A `sessions` row is not a stable identity: `move_session` creates a new
//! row on the target host and kills the source, so anything pointing at a
//! session id is orphaned by a move. A participant survives it — the move
//! re-points the participant instead (`service/move_session/finalise.rs`).

use super::*;
use crate::ipc_error::codes;

/// One addressable endpoint. `kind` is `session` | `client` | `hub`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParticipantRow {
    pub id: i64,
    pub kind: String,
    #[serde(default)]
    pub session_id: Option<i64>,
    #[serde(default)]
    pub client_id: Option<i64>,
    pub created_at: i64,
    /// Tombstone. A retired participant still resolves, and reports that it
    /// is gone, so a sender learns instead of timing out.
    #[serde(default)]
    pub retired_at: Option<i64>,
    /// The full `<fleet>/session/<host>/<name>` address of a `remote`
    /// participant (migration 054). `None` for every other kind.
    #[serde(default)]
    pub address: Option<String>,
    /// The `peer_links` row this `remote` participant belongs to (migration
    /// 054). `None` for every other kind.
    #[serde(default)]
    pub peer_link_id: Option<i64>,
}

/// A participant standing for a session on another, linked hub (migration
/// 054) — see `store::peer_links`.
pub const PARTICIPANT_REMOTE: &str = "remote";

const COLUMNS: &str =
    "id, kind, session_id, client_id, created_at, retired_at, address, peer_link_id";

/// How long a tombstoned participant's undelivered mail is kept. Long enough
/// that a sender waiting on a reply, or a human reading a timeline the next
/// day, still sees why nothing came back.
pub const RETIRED_RETENTION_SECS: i64 = 7 * 24 * 60 * 60;

fn map(row: &rusqlite::Row<'_>) -> rusqlite::Result<ParticipantRow> {
    Ok(ParticipantRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        session_id: row.get(2)?,
        client_id: row.get(3)?,
        created_at: row.get(4)?,
        retired_at: row.get(5)?,
        address: row.get(6)?,
        peer_link_id: row.get(7)?,
    })
}

impl Store {
    /// The participant id for `session_id`, creating it when absent. The
    /// unique partial index on `participants(session_id)` makes a concurrent
    /// double-create impossible, so this never forks an identity.
    pub fn ensure_participant_for_session(
        &self,
        session_id: i64,
    ) -> Result<i64, crate::ipc_error::IpcError> {
        if let Some(p) = self.participant_for_session(session_id)? {
            return Ok(p.id);
        }
        self.conn.execute(
            "INSERT INTO participants (kind, session_id, created_at) VALUES ('session', ?1, ?2)",
            rusqlite::params![session_id, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn participant_by_id(
        &self,
        id: i64,
    ) -> Result<Option<ParticipantRow>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM participants WHERE id = ?1"),
                rusqlite::params![id],
                map,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    pub fn participant_for_session(
        &self,
        session_id: i64,
    ) -> Result<Option<ParticipantRow>, crate::ipc_error::IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM participants WHERE session_id = ?1"),
                rusqlite::params![session_id],
                map,
            )
            .optional()
            .map_err(crate::ipc_error::IpcError::from)
    }

    /// Follow a moved session: the identity (and therefore every message
    /// addressed to it) now points at `new_session_id`. Refused for a retired
    /// participant — a tombstone never comes back to life.
    ///
    /// `insert_message` calls `ensure_participant_for_session`, so
    /// `new_session_id` (the move's target row) may already own a DIFFERENT
    /// participant by the time a move finalises — e.g. someone addressed a
    /// message to the target session before the move's source was killed.
    /// The unique partial index on `participants(session_id)` allows only
    /// one live owner, so that participant is folded into the one being
    /// re-pointed here rather than left to collide: every message it sent or
    /// received is reassigned to `participant_id`, and it is retired (never
    /// deleted, consistent with the rest of this module) before the claim on
    /// `new_session_id` is made. The re-pointed identity — the one carrying
    /// this session's history across the move — is the survivor.
    pub fn repoint_participant(
        &self,
        participant_id: i64,
        new_session_id: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let row = self.participant_by_id(participant_id)?.ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                codes::E_PARTICIPANT_UNKNOWN,
                format!("participant {participant_id} not found"),
            )
        })?;
        if row.retired_at.is_some() {
            return Err(crate::ipc_error::IpcError::new(
                codes::E_PARTICIPANT_RETIRED,
                format!("participant {participant_id} is retired"),
            ));
        }
        // Fix round 1, Important 3: the collision merge below is three
        // statements (two message re-assignments plus a retire) followed by
        // the claim on `new_session_id` — one transaction, or a failure
        // partway through could leave the collision's identity retired
        // while the survivor still doesn't own `new_session_id`; the
        // source's own later reap would then tombstone the survivor too,
        // and the merged mail would be swept in 7 days with no sender ever
        // told. `tx` rolls back automatically (rusqlite's `Drop`) on any
        // `?` return before `commit()`, matching the four tombstone sites
        // that already run inside their caller's transaction.
        let tx = self.conn.unchecked_transaction()?;
        if let Some(existing) = self.participant_for_session(new_session_id)? {
            if existing.id != participant_id {
                self.conn.execute(
                    "UPDATE session_messages SET to_participant_id = ?1 WHERE to_participant_id = ?2",
                    rusqlite::params![participant_id, existing.id],
                )?;
                self.conn.execute(
                    "UPDATE session_messages SET from_participant_id = ?1 WHERE from_participant_id = ?2",
                    rusqlite::params![participant_id, existing.id],
                )?;
                // Review C8: the collision's live work links move to the
                // survivor too (never as a second primary, never a duplicate
                // of a target the survivor already links); what is left ends
                // with the retire below, as history.
                self.conn.execute(
                    "UPDATE work_links SET participant_id = ?1, is_primary = is_primary AND NOT \
                       EXISTS(SELECT 1 FROM work_links s WHERE s.participant_id = ?1 \
                              AND s.ended_at IS NULL AND s.is_primary = 1) \
                     WHERE participant_id = ?2 AND ended_at IS NULL AND NOT EXISTS( \
                       SELECT 1 FROM work_links s WHERE s.participant_id = ?1 \
                         AND s.ended_at IS NULL AND s.item_id IS work_links.item_id \
                         AND s.ref_key IS work_links.ref_key)",
                    rusqlite::params![participant_id, existing.id],
                )?;
                self.retire_participant(existing.id)?;
            }
        }
        self.conn.execute(
            "UPDATE participants SET session_id = ?1 WHERE id = ?2",
            rusqlite::params![new_session_id, participant_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Tombstone, never delete: undelivered messages stay addressable for the
    /// GC window and the sender gets `message_undeliverable` rather than
    /// silence.
    ///
    /// Clears BOTH `session_id` and `client_id`, not just whichever one this
    /// participant happened to hold. Migration 043 declares a unique partial
    /// index on each column; a retired participant that kept a `client_id`
    /// would collide with the next retired one the moment `kind='client'`
    /// participants exist. Nothing creates those yet, so the collision is not
    /// reachable today — clearing both makes the index invariant hold by
    /// construction rather than by nobody having exercised it.
    pub fn retire_participant(
        &self,
        participant_id: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
             WHERE id = ?2 AND retired_at IS NULL",
            rusqlite::params![now_unix(), participant_id],
        )?;
        Ok(())
    }

    /// Consecutive `Stop` blocks this session currently sits behind.
    pub fn stop_block_streak(&self, session_id: i64) -> Result<u32, crate::ipc_error::IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(stop_block_streak, 0) FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    pub fn bump_stop_block_streak(
        &self,
        session_id: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE sessions SET stop_block_streak = COALESCE(stop_block_streak, 0) + 1 \
             WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Called when a turn ends without a block, so a later question is not
    /// punished for an earlier streak.
    pub fn reset_stop_block_streak(
        &self,
        session_id: i64,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.conn.execute(
            "UPDATE sessions SET stop_block_streak = 0 WHERE id = ?1 AND stop_block_streak <> 0",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// Drop the mail of participants retired longer than `older_than_secs`
    /// (measured from `now`), telling each SENDER about anything that was
    /// never read. Returns how many participants were swept.
    ///
    /// `now` is taken explicitly rather than read from the wall clock inside
    /// — the same parameter `service::gc::sweep_with` already threads
    /// through for its session sweep, so a caller (and its tests) can pick
    /// an exact instant instead of this racing the real clock.
    ///
    /// Also sweeps `session_messages` rows with a NULL `to_participant_id`:
    /// migration 043's backfill left those NULL for any pre-existing message
    /// whose session no longer existed, and such a row can never be
    /// delivered (delivery resolves through the participant) or, without
    /// this clause, ever be swept either. Those are aged on `sent_at` since
    /// there is no participant to retire.
    pub fn sweep_retired_participants(
        &self,
        now: i64,
        older_than_secs: i64,
    ) -> Result<usize, crate::ipc_error::IpcError> {
        let cutoff = now - older_than_secs;

        // Defence in depth behind `insert_message`'s existence check (final
        // review, Important 2): retire any LIVE participant whose session
        // row is gone. Nothing else ever would — `delete_session` tombstones
        // the participant it knows about, so a participant left pointing at
        // a vanished row (an older store, a raw SQL path, a future caller
        // that skips `insert_message`) is unreachable: the sweep below only
        // looks at retired rows, so its mail would sit forever and its
        // senders would never be told. Retiring it starts the ordinary
        // window; a later sweep then reports and drops it. `retired_at` is
        // `now`, not the cutoff, so this never deletes mail in the same pass
        // that discovers the orphan.
        self.conn.execute(
            "UPDATE participants SET retired_at = ?1, session_id = NULL, client_id = NULL \
             WHERE retired_at IS NULL AND session_id IS NOT NULL \
               AND session_id NOT IN (SELECT id FROM sessions)",
            rusqlite::params![now],
        )?;

        // Orphaned rows first: no participant to sweep by, only messages.
        let orphans: Vec<(i64, i64, bool)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, from_session_id, read_at IS NULL FROM session_messages \
                 WHERE to_participant_id IS NULL AND sent_at <= ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![cutoff], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (mid, sender, unread) in &orphans {
            if *unread && *sender != 0 {
                let _ = self.insert_session_event(
                    *sender,
                    "message_undeliverable",
                    Some(&format!("message {mid} was never read; recipient is gone")),
                );
            }
        }
        if !orphans.is_empty() {
            self.conn.execute(
                "DELETE FROM session_messages WHERE to_participant_id IS NULL AND sent_at <= ?1",
                rusqlite::params![cutoff],
            )?;
        }

        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM participants WHERE retired_at IS NOT NULL AND retired_at <= ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![cutoff], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for pid in &ids {
            // Tell each sender about mail that was never read. A message
            // that WAS read is swept quietly — nothing went wrong there.
            let unread: Vec<(i64, i64)> = {
                let mut stmt = self.conn.prepare(
                    "SELECT id, from_session_id FROM session_messages \
                     WHERE to_participant_id = ?1 AND read_at IS NULL",
                )?;
                let rows =
                    stmt.query_map(rusqlite::params![pid], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (mid, sender) in unread {
                if sender != 0 {
                    let _ = self.insert_session_event(
                        sender,
                        "message_undeliverable",
                        Some(&format!("message {mid} was never read; recipient is gone")),
                    );
                }
            }
            self.conn.execute(
                "DELETE FROM session_messages WHERE to_participant_id = ?1",
                rusqlite::params![pid],
            )?;
            self.conn.execute(
                "DELETE FROM participants WHERE id = ?1",
                rusqlite::params![pid],
            )?;
        }
        Ok(ids.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// Migration 045: identity is minted with the row, not on first mail, so
    /// anything anchored on the participant (a move's re-point, work links)
    /// never finds a session without one.
    #[test]
    fn every_new_session_row_has_a_live_participant_at_once() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "fresh");
        let p = s
            .participant_for_session(sid)
            .unwrap()
            .expect("minted on insert");
        assert_eq!(p.kind, "session");
        assert_eq!(p.retired_at, None);
        assert_eq!(
            s.ensure_participant_for_session(sid).unwrap(),
            p.id,
            "ensure finds the minted identity rather than forking a second"
        );
    }

    /// `sessions.id` is reused. The dead session's participant was retired
    /// (and unbound) by the delete, so the row that takes its id gets a new
    /// identity — never the dead one's mail.
    #[test]
    fn a_reused_session_id_gets_a_fresh_identity() {
        let s = Store::open_in_memory().unwrap();
        let first = seed(&s, "first");
        let dead = s.participant_for_session(first).unwrap().unwrap().id;
        s.delete_session(first).unwrap();
        let second = seed(&s, "second");
        let p = s.participant_for_session(second).unwrap().expect("minted");
        assert_ne!(p.id, dead);
        assert!(s
            .participant_by_id(dead)
            .unwrap()
            .unwrap()
            .retired_at
            .is_some());
    }

    #[test]
    fn ensure_is_idempotent_and_returns_the_same_id() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "alpha");
        let a = s.ensure_participant_for_session(sid).unwrap();
        let b = s.ensure_participant_for_session(sid).unwrap();
        assert_eq!(a, b, "a second ensure must not create a second identity");
        let row = s.participant_by_id(a).unwrap().unwrap();
        assert_eq!(row.kind, "session");
        assert_eq!(row.session_id, Some(sid));
        assert_eq!(row.retired_at, None);
    }

    #[test]
    fn repoint_moves_the_identity_to_a_new_session_row() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        let p = s.ensure_participant_for_session(src).unwrap();
        s.repoint_participant(p, dst).unwrap();
        assert_eq!(
            s.participant_by_id(p).unwrap().unwrap().session_id,
            Some(dst)
        );
        // The identity did not fork: the old session no longer owns one.
        assert!(s.participant_for_session(src).unwrap().is_none());
        assert_eq!(s.participant_for_session(dst).unwrap().unwrap().id, p);
    }

    #[test]
    fn retire_tombstones_rather_than_deletes() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "gone");
        let p = s.ensure_participant_for_session(sid).unwrap();
        s.retire_participant(p).unwrap();
        let row = s.participant_by_id(p).unwrap().expect("row still resolves");
        assert!(
            row.retired_at.is_some(),
            "retired participants must still resolve"
        );
    }

    /// `insert_message` auto-creates a participant for its recipient, so the
    /// move's target row can already own one by the time `finalise` calls
    /// `repoint_participant` — e.g. someone messaged the target session
    /// before the source was killed. Without a collision guard this trips
    /// the unique partial index on `participants(session_id)` as a bare
    /// `E_SQLITE`. Ruling 4: merge the collision's mail into the surviving
    /// (re-pointed) identity and retire the collided-with participant.
    #[test]
    fn repointing_into_a_session_that_already_has_a_participant_merges_and_retires_it() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let src = seed(&s, "worker");
        let dst = seed(&s, "worker-moved");

        // History addressed to the source before the move.
        let m1 = s
            .insert_message(a, src, "before the move", "message", None)
            .unwrap();
        let src_p = s.participant_for_session(src).unwrap().unwrap().id;

        // A message lands on the target row before finalise re-points —
        // this is what creates the collision.
        let m2 = s
            .insert_message(a, dst, "already there", "message", None)
            .unwrap();
        let dst_p = s.participant_for_session(dst).unwrap().unwrap().id;
        assert_ne!(
            src_p, dst_p,
            "two distinct participants exist before the repoint"
        );

        s.repoint_participant(src_p, dst).unwrap();

        // The surviving identity is the re-pointed one, now owning `dst`.
        assert_eq!(
            s.participant_by_id(src_p).unwrap().unwrap().session_id,
            Some(dst)
        );
        // The collided-with participant is tombstoned, not deleted.
        let retired = s.participant_by_id(dst_p).unwrap().unwrap();
        assert!(retired.retired_at.is_some(), "the collision is tombstoned");
        assert_eq!(retired.session_id, None);
        // Both messages now resolve through the surviving identity.
        let pending = s.list_undelivered_for_session(dst, 10).unwrap();
        let mut ids: Vec<_> = pending.iter().map(|m| m.id).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![m1, m2],
            "both the old and the colliding mail follow the survivor"
        );
    }

    /// Final review, Important 2 — defence in depth behind
    /// `insert_message`'s existence check. Whatever the route (an older store
    /// written before that check, a raw SQL path, a future caller), a LIVE
    /// participant whose `session_id` resolves to no session row is
    /// unreachable: nothing retires it, so the retention sweep never reaches
    /// it and its senders are never told. The sweep tombstones it, which
    /// starts the normal 7-day window and ends in a
    /// `message_undeliverable` on each sender's timeline.
    #[test]
    fn the_sweep_retires_a_participant_whose_session_is_gone() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let p = s.ensure_participant_for_session(b).unwrap();
        // Orphan it the way only a bug (or an older store) can: the row goes
        // away without `delete_session`'s tombstone.
        s.conn
            .execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![b])
            .unwrap();
        assert!(
            s.participant_by_id(p)
                .unwrap()
                .unwrap()
                .retired_at
                .is_none(),
            "the premise: it is still live"
        );

        let now = now_unix();
        s.sweep_retired_participants(now, RETIRED_RETENTION_SECS)
            .unwrap();
        let row = s.participant_by_id(p).unwrap().expect("still resolves");
        assert!(
            row.retired_at.is_some(),
            "a participant with no session must be retired by the sweep"
        );
        assert_eq!(row.session_id, None);
        let _ = a;
    }

    #[test]
    fn repointing_a_retired_participant_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let p = s.ensure_participant_for_session(a).unwrap();
        s.retire_participant(p).unwrap();
        let err = s.repoint_participant(p, b).unwrap_err();
        assert_eq!(err.code, "E_PARTICIPANT_RETIRED");
    }

    /// Migration 043 declares a unique partial index on `client_id` (as it
    /// does on `session_id`): retiring must clear both columns, not just
    /// `session_id`, so a later retired participant can never collide on a
    /// leftover `client_id`. Nothing in this cycle creates a `kind='client'`
    /// participant, so we set `client_id` directly to exercise the
    /// invariant.
    #[test]
    fn retire_clears_both_session_id_and_client_id() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "both-cols");
        let p = s.ensure_participant_for_session(sid).unwrap();
        s.conn
            .execute(
                "UPDATE participants SET client_id = ?1 WHERE id = ?2",
                rusqlite::params![999_i64, p],
            )
            .unwrap();

        s.retire_participant(p).unwrap();

        let row = s.participant_by_id(p).unwrap().unwrap();
        assert_eq!(row.session_id, None, "session_id must be cleared");
        assert_eq!(row.client_id, None, "client_id must be cleared too");
    }

    #[test]
    fn a_retired_participant_within_the_window_is_kept() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "pending", "message", None).unwrap();
        s.delete_session(b).unwrap();
        assert_eq!(
            s.sweep_retired_participants(now_unix(), RETIRED_RETENTION_SECS)
                .unwrap(),
            0
        );
        assert!(s.get_message(m).unwrap().is_some());
    }

    #[test]
    fn past_the_window_the_mail_is_swept_and_the_sender_is_told() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "pending", "message", None).unwrap();
        s.delete_session(b).unwrap();
        // Age the tombstone past the window.
        s.conn
            .execute(
                "UPDATE participants SET retired_at = retired_at - ?1 WHERE retired_at IS NOT NULL",
                rusqlite::params![RETIRED_RETENTION_SECS + 60],
            )
            .unwrap();

        assert_eq!(
            s.sweep_retired_participants(now_unix(), RETIRED_RETENTION_SECS)
                .unwrap(),
            1
        );
        assert!(s.get_message(m).unwrap().is_none(), "the mail is gone");
        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the SENDER must learn its message was never read: {ev:?}"
        );
    }

    #[test]
    fn a_read_message_is_swept_without_telling_the_sender_anything() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "read me", "message", None).unwrap();
        s.mark_messages_read(&[m], b).unwrap();
        s.delete_session(b).unwrap();
        s.conn
            .execute(
                "UPDATE participants SET retired_at = retired_at - ?1 WHERE retired_at IS NOT NULL",
                rusqlite::params![RETIRED_RETENTION_SECS + 60],
            )
            .unwrap();
        s.sweep_retired_participants(now_unix(), RETIRED_RETENTION_SECS)
            .unwrap();
        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            !ev.iter().any(|e| e.kind == "message_undeliverable"),
            "a message that WAS read is not undeliverable"
        );
    }

    /// Migration 043's backfill left `to_participant_id` NULL for any
    /// pre-existing message whose session no longer existed at migration
    /// time. Such a row can never be delivered (delivery resolves through
    /// the participant) and, without a dedicated clause, could never be
    /// swept either — immortal garbage in an upgraded database. Simulated
    /// here directly since nothing in the current schema produces a NULL
    /// `to_participant_id` through the public API anymore.
    #[test]
    fn an_orphaned_row_with_no_participant_is_swept_and_the_sender_is_told_when_unread() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s.insert_message(a, b, "orphaned", "message", None).unwrap();
        s.conn
            .execute(
                "UPDATE session_messages SET to_participant_id = NULL, \
                 sent_at = sent_at - ?1 WHERE id = ?2",
                rusqlite::params![RETIRED_RETENTION_SECS + 60, m],
            )
            .unwrap();

        s.sweep_retired_participants(now_unix(), RETIRED_RETENTION_SECS)
            .unwrap();

        assert!(
            s.get_message(m).unwrap().is_none(),
            "an orphaned row must not be immortal garbage"
        );
        let ev = s.list_session_events(a, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the sender must learn its orphaned message was never read: {ev:?}"
        );
    }

    #[test]
    fn a_recent_orphaned_row_is_kept() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "alpha");
        let b = seed(&s, "beta");
        let m = s
            .insert_message(a, b, "orphaned but fresh", "message", None)
            .unwrap();
        s.conn
            .execute(
                "UPDATE session_messages SET to_participant_id = NULL WHERE id = ?1",
                rusqlite::params![m],
            )
            .unwrap();

        s.sweep_retired_participants(now_unix(), RETIRED_RETENTION_SECS)
            .unwrap();

        assert!(s.get_message(m).unwrap().is_some());
    }
}
