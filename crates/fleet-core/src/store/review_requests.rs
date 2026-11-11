//! Review requests (migration 225): "Ask for review…" puts "X asks you to
//! review" in a colleague's Inbox; the share it comes with is an ordinary
//! grant. What an ask does end to end is `service::sessions::sharing`
//! (`ask_for_review`, `review_request`).

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// A note on an ask longer than this is refused.
pub const REVIEW_REQUEST_NOTE_MAX: usize = 2000;

/// One ask, with both people's names as the Inbox shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequestRow {
    pub id: i64,
    pub session_id: i64,
    pub from_person_id: i64,
    /// The asker's display name (or name).
    pub from_name: String,
    pub to_person_id: i64,
    pub to_name: String,
    /// drive: may send notes to the agent; watch: reads and drafts only.
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
    /// The session's caption (its friendly name, else its tmux name), so a
    /// client need not find the session in its own list to title the ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
}

const SELECT: &str = "SELECT r.id, r.session_id, r.from_person_id, \
       COALESCE(f.display_name, f.name, ''), r.to_person_id, \
       COALESCE(t.display_name, t.name, ''), r.level, r.note, r.created_at, r.done_at, \
       COALESCE(s.friendly_name, s.tmux_name) \
  FROM review_requests r \
  LEFT JOIN people f ON f.id = r.from_person_id \
  LEFT JOIN people t ON t.id = r.to_person_id \
  LEFT JOIN sessions s ON s.id = r.session_id";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewRequestRow> {
    Ok(ReviewRequestRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        from_person_id: r.get(2)?,
        from_name: r.get(3)?,
        to_person_id: r.get(4)?,
        to_name: r.get(5)?,
        level: r.get(6)?,
        note: r.get(7)?,
        created_at: r.get(8)?,
        done_at: r.get(9)?,
        session_name: r.get(10)?,
    })
}

impl Store {
    /// Ask `to` to review `session_id` on `from`'s behalf. An open ask to the
    /// same person on the same session is refreshed (note, level, time),
    /// never doubled.
    pub fn ask_for_review(
        &self,
        session_id: i64,
        from: i64,
        to: i64,
        level: &str,
        note: Option<&str>,
        now: i64,
    ) -> Result<ReviewRequestRow, IpcError> {
        self.conn.execute(
            "INSERT INTO review_requests \
               (session_id, from_person_id, to_person_id, level, note, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (session_id, to_person_id) WHERE done_at IS NULL DO UPDATE SET \
               from_person_id = excluded.from_person_id, level = excluded.level, \
               note = excluded.note, created_at = excluded.created_at",
            rusqlite::params![session_id, from, to, level, note, now],
        )?;
        Ok(self.conn.query_row(
            &format!(
                "{SELECT} WHERE r.session_id = ?1 AND r.to_person_id = ?2 AND r.done_at IS NULL"
            ),
            rusqlite::params![session_id, to],
            row,
        )?)
    }

    /// The asks addressed to `person`: open ones, newest first, and with
    /// `done` also those finished in the last week.
    pub fn review_requests_for(
        &self,
        person: i64,
        done: bool,
        now: i64,
    ) -> Result<Vec<ReviewRequestRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT} WHERE r.to_person_id = ?1 \
               AND (r.done_at IS NULL OR (?2 AND r.done_at >= ?3)) \
             ORDER BY r.done_at IS NOT NULL, r.created_at DESC, r.id DESC LIMIT 100"
        ))?;
        let mut rows = stmt
            .query_map(rusqlite::params![person, done, now - 7 * 86_400], row)?
            .collect::<Result<Vec<_>, _>>()?;
        // An open ask shows the level the person holds now: a share narrowed
        // since the ask must not still read "can send notes". (An ask whose
        // share is gone keeps its stored level; what to show for it is open.)
        if rows.iter().any(|r| r.done_at.is_none()) {
            let live = self.grants_for_person(person)?;
            for r in rows.iter_mut().filter(|r| r.done_at.is_none()) {
                if let Some(level) = live.get(&r.session_id) {
                    r.level.clone_from(level);
                }
            }
        }
        Ok(rows)
    }

    /// Mark ask `id` done (or open again) — only by the person it asks.
    /// `None` when there is no such ask to them.
    pub fn finish_review_request(
        &self,
        id: i64,
        person: i64,
        done: bool,
        now: i64,
    ) -> Result<Option<ReviewRequestRow>, IpcError> {
        // One open ask per session and person (a partial unique index):
        // reopening an older one while a newer one is open is refused in
        // words rather than as the index's raw constraint error.
        if !done {
            let newer_open: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM review_requests o \
                   JOIN review_requests r ON r.id = ?1 AND r.to_person_id = ?2 \
                  WHERE o.session_id = r.session_id AND o.to_person_id = r.to_person_id \
                    AND o.id <> r.id AND o.done_at IS NULL)",
                rusqlite::params![id, person],
                |r| r.get(0),
            )?;
            if newer_open {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "a newer ask for review of this session is open; review request {id} \
                         stays done"
                    ),
                ));
            }
        }
        let n = self.conn.execute(
            "UPDATE review_requests SET done_at = ?3 WHERE id = ?1 AND to_person_id = ?2",
            rusqlite::params![id, person, done.then_some(now)],
        )?;
        if n == 0 {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(&format!("{SELECT} WHERE r.id = ?1"), [id], row)
            .optional()?)
    }

    /// Close the open ask to `person` on `session_id`, if any: the share it
    /// came with was revoked (`revoke_session_grant_to`). Returns how many
    /// it closed (0 or 1).
    pub(crate) fn close_review_requests_to(
        &self,
        session_id: i64,
        person: i64,
        now: i64,
    ) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "UPDATE review_requests SET done_at = ?3 \
              WHERE session_id = ?1 AND to_person_id = ?2 AND done_at IS NULL",
            rusqlite::params![session_id, person, now],
        )?)
    }

    /// Whether `person` is only ever a viewer: they hold live org
    /// memberships and every one of them is `viewer`. A person in no org is
    /// not (a one-person fleet's colleague is added as a person, not a role).
    pub fn person_is_viewer_only(&self, person: i64) -> Result<bool, IpcError> {
        let (all, viewer): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(role = 'viewer'), 0) FROM org_members \
              WHERE person_id = ?1 AND removed_at IS NULL",
            [person],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(all > 0 && all == viewer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Store, i64, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let sid = s
            .upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let bo = s.create_person("bo", None).unwrap().id;
        (s, sid, ada, bo)
    }

    #[test]
    fn an_ask_is_refreshed_not_doubled_and_the_colleague_finishes_it() {
        let (s, sid, ada, bo) = setup();
        let a = s
            .ask_for_review(sid, ada, bo, "drive", Some("the retry loop"), 10)
            .unwrap();
        assert_eq!((a.from_name.as_str(), a.to_name.as_str()), ("ada", "bo"));
        let b = s.ask_for_review(sid, ada, bo, "watch", None, 20).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(
            (b.level.as_str(), b.note, b.created_at),
            ("watch", None, 20)
        );
        assert_eq!(s.review_requests_for(bo, false, 30).unwrap().len(), 1);
        assert!(s.review_requests_for(ada, false, 30).unwrap().is_empty());
        // Only the person asked marks it done.
        assert!(s
            .finish_review_request(a.id, ada, true, 40)
            .unwrap()
            .is_none());
        let done = s
            .finish_review_request(a.id, bo, true, 40)
            .unwrap()
            .unwrap();
        assert_eq!(done.done_at, Some(40));
        assert!(s.review_requests_for(bo, false, 50).unwrap().is_empty());
        assert_eq!(s.review_requests_for(bo, true, 50).unwrap().len(), 1);
        // A new ask after a done one is a new row.
        let c = s.ask_for_review(sid, ada, bo, "drive", None, 60).unwrap();
        assert_ne!(c.id, a.id);
    }

    /// Review finding 4: reopening an older ask while a newer one is open is
    /// a plain `E_INVALID_STATE`, not the unique index's raw error; and an
    /// ask carries its session's caption.
    #[test]
    fn reopening_under_a_newer_open_ask_is_refused_in_words() {
        let (s, sid, ada, bo) = setup();
        let a = s.ask_for_review(sid, ada, bo, "drive", None, 10).unwrap();
        assert_eq!(a.session_name.as_deref(), Some("api-1"));
        s.finish_review_request(a.id, bo, true, 20).unwrap();
        let b = s.ask_for_review(sid, ada, bo, "drive", None, 30).unwrap();
        let e = s.finish_review_request(a.id, bo, false, 40).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE, "{}", e.message);
        // With the newer one done, the older one reopens.
        s.finish_review_request(b.id, bo, true, 50).unwrap();
        let again = s
            .finish_review_request(a.id, bo, false, 60)
            .unwrap()
            .unwrap();
        assert_eq!(again.done_at, None);
    }

    /// Review finding 4: revoking the share an ask came with closes the ask.
    #[test]
    fn revoking_the_share_closes_the_ask() {
        let (s, sid, ada, bo) = setup();
        s.conn
            .execute(
                "UPDATE sessions SET owner_person_id = ?2, visibility = 'private' WHERE id = ?1",
                rusqlite::params![sid, ada],
            )
            .unwrap();
        s.grant_session(sid, crate::store::GrantRecipient::Person(bo), "drive", ada)
            .unwrap();
        s.ask_for_review(sid, ada, bo, "drive", None, 10).unwrap();
        assert_eq!(s.review_requests_for(bo, false, 20).unwrap().len(), 1);
        s.revoke_session_grant(sid, bo, ada).unwrap();
        assert!(s.review_requests_for(bo, false, 30).unwrap().is_empty());
    }

    #[test]
    fn only_a_viewer_everywhere_is_a_viewer() {
        let (s, _, ada, _) = setup();
        assert!(!s.person_is_viewer_only(ada).unwrap());
        s.conn
            .execute(
                "INSERT INTO org_members (org_id, person_id, role, added_at) VALUES (1, ?1, 'viewer', 0)",
                [ada],
            )
            .unwrap();
        assert!(s.person_is_viewer_only(ada).unwrap());
        s.conn
            .execute(
                "INSERT INTO org_members (org_id, person_id, role, added_at) VALUES (2, ?1, 'member', 0)",
                [ada],
            )
            .unwrap();
        assert!(!s.person_is_viewer_only(ada).unwrap());
    }
}
