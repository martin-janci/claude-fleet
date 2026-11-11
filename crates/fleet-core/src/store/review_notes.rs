//! Review notes (migration 223): a person's notes on lines of a session's
//! diff, stored once they are sent to the session's agent. What a send types
//! is `service::sessions::review_notes`.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// At most this many notes in one send.
pub const REVIEW_NOTES_MAX: usize = 50;
/// A note's body or path longer than this many characters is refused.
pub const REVIEW_NOTE_MAX: usize = 4000;
/// A quoted line is cut to this many characters (and an ellipsis): it only
/// shows the agent where the note is, and a whole minified line must not
/// make the note unsendable.
pub const REVIEW_QUOTE_MAX: usize = 300;

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
    /// Absent once the send reached the session. Otherwise: `waiting` (its
    /// prompt is kept for the session's next idle moment), `not_delivered`
    /// (that prompt was cancelled, dropped or failed: the agent never got
    /// these notes) or `uncertain` (the typing may or may not have landed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// Why, for `not_delivered` / `uncertain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
    /// The kept prompt carrying the send (`queued_prompts` / Send later),
    /// when it was kept rather than typed at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_id: Option<i64>,
}

/// `quote` cut to [`REVIEW_QUOTE_MAX`] characters, at a char boundary, with
/// no mark: a client places a note where the row's text, cut the same way
/// (`QUOTE_MAX` in `src/lib/review_notes.ts`), equals it.
fn cut_quote(q: &str) -> String {
    match q.char_indices().nth(REVIEW_QUOTE_MAX) {
        Some((at, _)) => q[..at].to_string(),
        None => q.to_string(),
    }
}

/// Check what a device sent: a path inside the checkout, a known side, a
/// body, sizes in bounds. Returns the notes trimmed, quotes cut, in the
/// order the prompt numbers them: by path, then line (whole-file first).
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
        let chars = |s: &str| s.chars().count();
        if chars(&path) > REVIEW_NOTE_MAX || chars(&body) > REVIEW_NOTE_MAX {
            return Err(bad(format!(
                "a note on {path} is too long: at most {REVIEW_NOTE_MAX} characters"
            )));
        }
        let quote = n
            .quote
            .as_deref()
            .map(str::trim_end)
            .filter(|q| !q.trim().is_empty())
            .map(cut_quote);
        out.push(NewReviewNote {
            path,
            line: n.line,
            side: n.side.clone(),
            quote,
            body,
        });
    }
    // Stable: notes on one line keep the order they were sent in.
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(out)
}

/// The notes with their send's delivery: the kept prompt (migration 226) and
/// what became of it.
const FROM: &str = "review_notes n \
     LEFT JOIN review_note_queue q ON q.session_id = n.session_id AND q.batch = n.batch \
     LEFT JOIN deferred_prompts d ON d.id = q.deferred_prompt_id";

const COLS: &str = "n.id, n.session_id, n.batch, n.seq, n.path, n.line, n.side, n.quote, n.body, \
                    n.author, n.author_person_id, n.created_at, n.resolved_at, n.resolved_by, \
                    q.batch, q.deferred_prompt_id, q.error, d.id, d.delivered_at, d.failed_at, \
                    d.cancelled_at, d.skipped_at, d.error";

/// What became of a send, from its queue row and its kept prompt: `None`
/// when it reached the session (or there is nothing to say).
fn delivery(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Option<&'static str>, Option<String>)> {
    let queued: Option<String> = r.get(14)?;
    if queued.is_none() {
        return Ok((None, None));
    }
    let kept: Option<i64> = r.get(15)?;
    let error: Option<String> = r.get(16)?;
    if kept.is_none() {
        // Typed at once, but the typing may not have landed.
        return Ok((Some("uncertain"), error));
    }
    let Some(_) = r.get::<_, Option<i64>>(17)? else {
        // The prompt row is gone (it stayed with a moved session's old row
        // once it had gone out or been settled): nothing more is known.
        return Ok((None, None));
    };
    let delivered: Option<i64> = r.get(18)?;
    let failed: Option<i64> = r.get(19)?;
    let cancelled: Option<i64> = r.get(20)?;
    let skipped: Option<i64> = r.get(21)?;
    let why: Option<String> = r.get(22)?;
    Ok(if cancelled.is_some() {
        (
            Some("not_delivered"),
            Some("its prompt was cancelled".into()),
        )
    } else if skipped.is_some() {
        (Some("not_delivered"), why)
    } else if failed.is_some() && delivered.is_some() {
        (Some("uncertain"), why)
    } else if failed.is_some() {
        (Some("not_delivered"), why)
    } else if delivered.is_none() {
        (Some("waiting"), None)
    } else {
        (None, None)
    })
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewNoteRow> {
    let (delivery, delivery_error) = delivery(r)?;
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
        delivery: delivery.map(str::to_string),
        delivery_error,
        queued_id: r.get(15)?,
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
            "SELECT {COLS} FROM {FROM} WHERE n.session_id = ?1 AND n.batch = ?2 ORDER BY n.seq"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, batch], row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Store send `batch`'s notes, numbered from 1, and answer whether they
    /// are new with the batch as stored. A batch already stored is left as
    /// it is and answered with `false`: the caller types nothing twice.
    pub fn insert_review_notes(
        &self,
        session_id: i64,
        batch: &str,
        notes: &[NewReviewNote],
        author: &str,
        author_person_id: Option<i64>,
        now: i64,
    ) -> Result<(bool, Vec<ReviewNoteRow>), IpcError> {
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
            drop(tx);
            return Ok((false, self.review_note_batch(session_id, batch)?));
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
        Ok((true, self.review_note_batch(session_id, batch)?))
    }

    /// Take back send `batch` (its prompt could not be typed or kept).
    pub fn delete_review_note_batch(&self, session_id: i64, batch: &str) -> Result<(), IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        for table in ["review_notes", "review_note_queue"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE session_id = ?1 AND batch = ?2"),
                rusqlite::params![session_id, batch],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Record that send `batch` did not simply reach the session: the prompt
    /// `kept` for its next idle moment, or (`kept` None) a typing that may
    /// not have landed, with `error`.
    pub fn set_review_note_queue(
        &self,
        session_id: i64,
        batch: &str,
        kept: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO review_note_queue (session_id, batch, deferred_prompt_id, error) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, batch, kept, error],
        )?;
        Ok(())
    }

    /// Move `from`'s review notes to `to` (a moved session's new row, or
    /// back on undo), beside [`Store::repoint_deferred_prompts`]. Answers
    /// how many notes moved.
    pub fn repoint_review_notes(&self, from: i64, to: i64) -> Result<usize, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE review_notes SET session_id = ?2 WHERE session_id = ?1",
            rusqlite::params![from, to],
        )?;
        tx.execute(
            "UPDATE review_note_queue SET session_id = ?2 WHERE session_id = ?1",
            rusqlite::params![from, to],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// A session's sent notes, newest send first, each send in its order.
    pub fn review_notes(&self, session_id: i64) -> Result<Vec<ReviewNoteRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM {FROM} WHERE n.session_id = ?1 \
              ORDER BY n.created_at DESC, n.batch, n.seq LIMIT 500"
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
                &format!("SELECT {COLS} FROM {FROM} WHERE n.id = ?1"),
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
        let (fresh, sent) = s
            .insert_review_notes(sid, "m1", &notes, "Ada · desktop", Some(2), 100)
            .unwrap();
        assert!(fresh);
        let (fresh, again) = s
            .insert_review_notes(sid, "m1", &notes, "Ada · desktop", Some(2), 101)
            .unwrap();
        assert!(!fresh);
        assert_eq!(again, sent, "a repeat answers the batch as stored");
        assert_eq!(sent, s.review_note_batch(sid, "m1").unwrap());
        assert!(sent
            .iter()
            .all(|n| n.delivery.is_none() && n.queued_id.is_none()));
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

    #[test]
    fn a_long_line_is_cut_and_sizes_count_characters() {
        // A minified line: the quote is cut at a char boundary, not refused.
        let long = NewReviewNote {
            quote: Some("é".repeat(5000)),
            ..note("min.js", Some(1), "x")
        };
        let ok = check_notes(&[long]).unwrap();
        let q = ok[0].quote.as_deref().unwrap();
        assert_eq!(q.chars().count(), REVIEW_QUOTE_MAX);
        let short = check_notes(&[note("a.rs", Some(1), "x")]).unwrap();
        assert_eq!(short[0].quote.as_deref(), Some("let base = 3;"));
        // 4000 two-byte characters is 8000 bytes, and still in bounds.
        let wide = note("a.rs", Some(1), &"é".repeat(REVIEW_NOTE_MAX));
        assert!(check_notes(&[wide]).is_ok());
        let over = note("a.rs", Some(1), &"x".repeat(REVIEW_NOTE_MAX + 1));
        assert_eq!(check_notes(&[over]).unwrap_err().code, codes::E_INVALID);
    }

    #[test]
    fn notes_are_numbered_by_path_then_line() {
        let ok = check_notes(&[
            note("b.rs", Some(9), "1"),
            note("a.rs", Some(30), "2"),
            note("a.rs", None, "3"),
            note("a.rs", Some(4), "4"),
            note("a.rs", Some(4), "5"),
        ])
        .unwrap();
        let bodies: Vec<&str> = ok.iter().map(|n| n.body.as_str()).collect();
        assert_eq!(bodies, ["3", "4", "5", "2", "1"]);
    }

    #[test]
    fn a_kept_send_reads_waiting_until_its_prompt_goes_or_is_taken_back() {
        let s = Store::open_in_memory().unwrap();
        let sid = session(&s);
        let state = |batch: &str| {
            let n = s.review_note_batch(sid, batch).unwrap().remove(0);
            (n.delivery, n.queued_id)
        };
        let notes = [note("a.rs", Some(1), "x")];
        for b in ["kept", "gone", "cancelled", "failed", "unsure"] {
            s.insert_review_notes(sid, b, &notes, "Ada", None, 1)
                .unwrap();
        }
        let kept = s.insert_deferred_prompt(sid, "p", 1).unwrap();
        s.set_review_note_queue(sid, "kept", Some(kept), None)
            .unwrap();
        assert_eq!(state("kept"), (Some("waiting".into()), Some(kept)));
        assert_eq!(state("gone"), (None, None), "typed at once: sent");
        // Typed later: sent.
        let gone = s.insert_deferred_prompt(sid, "p", 1).unwrap();
        s.set_review_note_queue(sid, "gone", Some(gone), None)
            .unwrap();
        assert!(s.claim_deferred_prompt(gone, 5).unwrap());
        assert_eq!(state("gone").0, None);
        // Taken back in Send later.
        let cancelled = s.insert_deferred_prompt(sid, "p", 1).unwrap();
        s.set_review_note_queue(sid, "cancelled", Some(cancelled), None)
            .unwrap();
        assert!(s.cancel_deferred_prompt(cancelled, 6).unwrap());
        assert_eq!(state("cancelled").0.as_deref(), Some("not_delivered"));
        // Its typing failed for good.
        let failed = s.insert_deferred_prompt(sid, "p", 1).unwrap();
        s.set_review_note_queue(sid, "failed", Some(failed), None)
            .unwrap();
        for _ in 0..crate::store::DEFERRED_MAX_ATTEMPTS {
            assert!(s.claim_deferred_prompt(failed, 7).unwrap());
            s.release_deferred_prompt(failed, "ssh down", 7).unwrap();
        }
        let n = s.review_note_batch(sid, "failed").unwrap().remove(0);
        assert_eq!(n.delivery.as_deref(), Some("not_delivered"));
        assert_eq!(n.delivery_error.as_deref(), Some("ssh down"));
        // Typed at once, but the typing may not have landed.
        s.set_review_note_queue(sid, "unsure", None, Some("timed out"))
            .unwrap();
        let n = s.review_note_batch(sid, "unsure").unwrap().remove(0);
        assert_eq!(n.delivery.as_deref(), Some("uncertain"));
        assert_eq!(n.delivery_error.as_deref(), Some("timed out"));
        // The listing says the same, and a take-back drops the queue row.
        assert_eq!(
            s.review_notes(sid)
                .unwrap()
                .iter()
                .filter(|n| n.delivery.is_some())
                .count(),
            4
        );
        s.delete_review_note_batch(sid, "kept").unwrap();
        s.insert_review_notes(sid, "kept", &notes, "Ada", None, 9)
            .unwrap();
        assert_eq!(state("kept"), (None, None));
    }

    #[test]
    fn notes_move_with_their_session() {
        let s = Store::open_in_memory().unwrap();
        let from = session(&s);
        let to = s
            .upsert_session("api-2", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let notes = [note("a.rs", Some(1), "x"), note("b.rs", Some(2), "y")];
        s.insert_review_notes(from, "m1", &notes, "Ada", None, 1)
            .unwrap();
        let kept = s.insert_deferred_prompt(from, "p", 1).unwrap();
        s.set_review_note_queue(from, "m1", Some(kept), None)
            .unwrap();
        s.repoint_deferred_prompts(from, to).unwrap();
        assert_eq!(s.repoint_review_notes(from, to).unwrap(), 2);
        assert!(s.review_notes(from).unwrap().is_empty());
        let moved = s.review_notes(to).unwrap();
        assert_eq!(moved.len(), 2);
        assert_eq!(moved[0].delivery.as_deref(), Some("waiting"));
        // The old row's delete no longer takes them.
        s.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [from])
            .unwrap();
        assert_eq!(s.review_notes(to).unwrap().len(), 2);
        // Undo moves them back.
        let back = s
            .upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap();
        assert_eq!(s.repoint_review_notes(to, back).unwrap(), 2);
        assert_eq!(s.review_notes(back).unwrap().len(), 2);
    }
}
