//! Review notes (migration 222): a person's notes on lines of a session's
//! diff, stored once they are sent to the session's agent. What a send types
//! is `service::sessions::review_notes`.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// At most this many notes in one send.
pub const REVIEW_NOTES_MAX: usize = 50;
/// A note's body, path or quote longer than this is refused.
pub const REVIEW_NOTE_MAX: usize = 4000;

/// One note as a device sends it.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct NewReviewNote {
    // Field docs are left off on purpose: they would ride every tool listing
    // (`the_served_definition_budget_stays_bounded`); `ReviewNotesParams`
    // names the shape once.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default = "new_side")]
    pub side: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub body: String,
}

fn new_side() -> String {
    "new".to_string()
}

/// A sent note.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNoteRow {
    pub id: i64,
    pub session_id: i64,
    /// The send's `client_msg_id`.
    pub batch: String,
    /// Its number in that send, as the prompt numbers it.
    pub seq: i64,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    pub side: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub body: String,
    /// Who sent it ("Ada · desktop").
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<i64>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
}

/// Check what a device sent: a path inside the checkout, a known side, a
/// body, sizes in bounds. Returns the notes trimmed.
pub fn check_notes(notes: &[NewReviewNote]) -> Result<Vec<NewReviewNote>, IpcError> {
    let bad = |m: String| IpcError::new(codes::E_INVALID, m);
    if notes.is_empty() {
        return Err(bad("no notes to send".into()));
    }
    if notes.len() > REVIEW_NOTES_MAX {
        return Err(bad(format!("at most {REVIEW_NOTES_MAX} notes in one send")));
    }
    let mut out = Vec::with_capacity(notes.len());
    for n in notes {
        let path = n.path.trim().trim_start_matches("./").to_string();
        let body = n.body.trim().to_string();
        if path.is_empty() || path.starts_with('/') || path.split('/').any(|p| p == "..") {
            return Err(bad(format!(
                "note path {:?} must be relative to the checkout",
                n.path
            )));
        }
        if body.is_empty() {
            return Err(bad(format!("the note on {path} is empty")));
        }
        if !matches!(n.side.as_str(), "new" | "old") {
            return Err(bad(format!("note side {:?}; new or old", n.side)));
        }
        if n.line.is_some_and(|l| l < 1) {
            return Err(bad("a note's line is 1 or more".into()));
        }
        let quote = n.quote.as_deref().map(str::trim_end).map(str::to_string);
        if path.len() > REVIEW_NOTE_MAX
            || body.len() > REVIEW_NOTE_MAX
            || quote.as_ref().is_some_and(|q| q.len() > REVIEW_NOTE_MAX)
        {
            return Err(bad(format!("a note on {path} is too long")));
        }
        out.push(NewReviewNote {
            path,
            line: n.line,
            side: n.side.clone(),
            quote: quote.filter(|q| !q.trim().is_empty()),
            body,
        });
    }
    Ok(out)
}

const COLS: &str = "id, session_id, batch, seq, path, line, side, quote, body, author, \
                    author_person_id, created_at, resolved_at, resolved_by";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewNoteRow> {
    Ok(ReviewNoteRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        batch: r.get(2)?,
        seq: r.get(3)?,
        path: r.get(4)?,
        line: r.get(5)?,
        side: r.get(6)?,
        quote: r.get(7)?,
        body: r.get(8)?,
        author: r.get(9)?,
        author_person_id: r.get(10)?,
        created_at: r.get(11)?,
        resolved_at: r.get(12)?,
        resolved_by: r.get(13)?,
    })
}

impl Store {
    /// The notes of send `batch` on `session_id`, in order; empty when there
    /// was no such send.
    pub fn review_note_batch(
        &self,
        session_id: i64,
        batch: &str,
    ) -> Result<Vec<ReviewNoteRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM review_notes WHERE session_id = ?1 AND batch = ?2 ORDER BY seq"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, batch], row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Store send `batch`'s notes, numbered from 1. A batch already stored is
    /// left as it is and `false` returned: the caller types nothing twice.
    pub fn insert_review_notes(
        &self,
        session_id: i64,
        batch: &str,
        notes: &[NewReviewNote],
        author: &str,
        author_person_id: Option<i64>,
        now: i64,
    ) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM review_notes WHERE session_id = ?1 AND batch = ?2 LIMIT 1",
                rusqlite::params![session_id, batch],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Ok(false);
        }
        for (i, n) in notes.iter().enumerate() {
            tx.execute(
                "INSERT INTO review_notes (session_id, batch, seq, path, line, side, quote, body, \
                                           author, author_person_id, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    session_id,
                    batch,
                    i as i64 + 1,
                    n.path,
                    n.line,
                    n.side,
                    n.quote,
                    n.body,
                    author,
                    author_person_id,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Take back send `batch` (its prompt could not be typed or kept).
    pub fn delete_review_note_batch(&self, session_id: i64, batch: &str) -> Result<(), IpcError> {
        self.conn.execute(
            "DELETE FROM review_notes WHERE session_id = ?1 AND batch = ?2",
            rusqlite::params![session_id, batch],
        )?;
        Ok(())
    }

    /// A session's sent notes, newest send first, each send in its order.
    pub fn review_notes(&self, session_id: i64) -> Result<Vec<ReviewNoteRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM review_notes WHERE session_id = ?1 \
              ORDER BY created_at DESC, batch, seq LIMIT 500"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id], row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark note `id` of `session_id` resolved (or open again). `None` when
    /// there is no such note on that session.
    pub fn resolve_review_note(
        &self,
        session_id: i64,
        id: i64,
        resolved: bool,
        by: &str,
        now: i64,
    ) -> Result<Option<ReviewNoteRow>, IpcError> {
        let (at, who) = if resolved {
            (Some(now), Some(by))
        } else {
            (None, None)
        };
        let n = self.conn.execute(
            "UPDATE review_notes SET resolved_at = ?3, resolved_by = ?4 \
              WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![id, session_id, at, who],
        )?;
        if n == 0 {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM review_notes WHERE id = ?1"),
                rusqlite::params![id],
                row,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, line: Option<i64>, body: &str) -> NewReviewNote {
        NewReviewNote {
            path: path.into(),
            line,
            side: "new".into(),
            quote: Some("let base = 3;  ".into()),
            body: body.into(),
        }
    }

    fn session(s: &Store) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    #[test]
    fn a_send_is_stored_once_and_resolved_by_a_person() {
        let s = Store::open_in_memory().unwrap();
        let sid = session(&s);
        let notes = vec![
            note("src/a.rs", Some(4), "Cap this"),
            note("b.md", None, "Typo"),
        ];
        assert!(s
            .insert_review_notes(sid, "m1", &notes, "Ada · desktop", Some(2), 100)
            .unwrap());
        assert!(!s
            .insert_review_notes(sid, "m1", &notes, "Ada · desktop", Some(2), 101)
            .unwrap());
        let sent = s.review_note_batch(sid, "m1").unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!((sent[0].seq, sent[1].seq), (1, 2));
        assert_eq!(sent[0].author, "Ada · desktop");
        let done = s
            .resolve_review_note(sid, sent[1].id, true, "Martin", 120)
            .unwrap()
            .unwrap();
        assert_eq!(done.resolved_at, Some(120));
        assert_eq!(done.resolved_by.as_deref(), Some("Martin"));
        let open = s
            .resolve_review_note(sid, sent[1].id, false, "Martin", 130)
            .unwrap()
            .unwrap();
        assert!(open.resolved_at.is_none() && open.resolved_by.is_none());
        assert!(s
            .resolve_review_note(sid + 9, sent[1].id, true, "x", 1)
            .unwrap()
            .is_none());
        assert_eq!(s.review_notes(sid).unwrap().len(), 2);
        s.delete_review_note_batch(sid, "m1").unwrap();
        assert!(s.review_notes(sid).unwrap().is_empty());
    }

    #[test]
    fn a_note_stays_inside_the_checkout_and_says_something() {
        let ok = check_notes(&[note("./src/a.rs", Some(1), "  x ")]).unwrap();
        assert_eq!(ok[0].path, "src/a.rs");
        assert_eq!(ok[0].body, "x");
        assert_eq!(ok[0].quote.as_deref(), Some("let base = 3;"));
        for bad in [
            vec![],
            vec![note("/etc/passwd", None, "x")],
            vec![note("../x", None, "x")],
            vec![note("a.rs", None, "  ")],
            vec![note("a.rs", Some(0), "x")],
            vec![NewReviewNote {
                side: "left".into(),
                ..note("a.rs", None, "x")
            }],
        ] {
            assert_eq!(check_notes(&bad).unwrap_err().code, codes::E_INVALID);
        }
        let many: Vec<_> = (0..=REVIEW_NOTES_MAX)
            .map(|i| note("a.rs", Some(i as i64 + 1), "x"))
            .collect();
        assert!(check_notes(&many).is_err());
    }
}
