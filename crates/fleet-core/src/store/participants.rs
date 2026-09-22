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
}

const COLUMNS: &str = "id, kind, session_id, client_id, created_at, retired_at";

fn map(row: &rusqlite::Row<'_>) -> rusqlite::Result<ParticipantRow> {
    Ok(ParticipantRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        session_id: row.get(2)?,
        client_id: row.get(3)?,
        created_at: row.get(4)?,
        retired_at: row.get(5)?,
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
        self.conn.execute(
            "UPDATE participants SET session_id = ?1 WHERE id = ?2",
            rusqlite::params![new_session_id, participant_id],
        )?;
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
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
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
}
