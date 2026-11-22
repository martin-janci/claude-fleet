//! Prompt authors (migration 224): who sent each prompt a hub tool typed or
//! queued, so the conversation can show a colleague's prompt as theirs.
//! The match of a turn to its row is `service::transcript`'s.

use super::Store;
use crate::ipc_error::IpcError;

/// How many characters of a prompt a row keeps to match it by.
pub const PROMPT_HEAD_CHARS: usize = 80;
/// Rows a session keeps; older ones are dropped as new ones come.
const KEEP_PER_SESSION: i64 = 500;

/// A stored author row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAuthorRow {
    pub at: i64,
    pub head: String,
    pub author: String,
    pub author_person_id: Option<i64>,
}

/// The head a prompt is matched by: whitespace folded to single spaces,
/// trimmed, cut to [`PROMPT_HEAD_CHARS`] characters. The transcript's text
/// and the typed text can differ in line endings and trailing space; this
/// folds both alike.
pub fn prompt_head(prompt: &str) -> String {
    prompt
        .split_whitespace()
        .flat_map(|w| std::iter::once(' ').chain(w.chars()))
        .skip(1)
        .take(PROMPT_HEAD_CHARS)
        .collect()
}

impl Store {
    /// Note that `author` sent `prompt` to `session_id` at `now`. An empty
    /// prompt is not recorded.
    pub fn record_prompt_author(
        &self,
        session_id: i64,
        prompt: &str,
        author: &str,
        author_person_id: Option<i64>,
        now: i64,
    ) -> Result<(), IpcError> {
        let head = prompt_head(prompt);
        if head.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO prompt_authors (session_id, at, head, author, author_person_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, now, head, author, author_person_id],
        )?;
        self.conn.execute(
            "DELETE FROM prompt_authors WHERE session_id = ?1 AND id <= \
               (SELECT id FROM prompt_authors WHERE session_id = ?1 \
                 ORDER BY id DESC LIMIT 1 OFFSET ?2)",
            rusqlite::params![session_id, KEEP_PER_SESSION],
        )?;
        Ok(())
    }

    /// Drop the row `queue_prompt` wrote for a queued prompt taken back
    /// before it went out (`body` as queued, `queued_at` its
    /// `deferred_prompts.created_at`): the row is written just after the
    /// queue row, so the oldest one of that head in the minute after it.
    /// Returns whether one was dropped.
    pub fn forget_prompt_author(
        &self,
        session_id: i64,
        body: &str,
        queued_at: i64,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM prompt_authors WHERE id = \
               (SELECT id FROM prompt_authors WHERE session_id = ?1 AND head = ?2 \
                  AND at >= ?3 AND at <= ?3 + 60 ORDER BY at, id LIMIT 1)",
            rusqlite::params![session_id, prompt_head(body), queued_at],
        )?;
        Ok(n > 0)
    }

    /// Move `from`'s author rows to `to` (a moved session's new row, or back
    /// on undo), beside [`Store::repoint_review_notes`]: the conversation
    /// goes on there, and the source row's delete would cascade them away,
    /// so a colleague's earlier prompts would read as the reader's own.
    /// Answers how many rows moved.
    pub fn repoint_prompt_authors(&self, from: i64, to: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "UPDATE prompt_authors SET session_id = ?2 WHERE session_id = ?1",
            rusqlite::params![from, to],
        )?)
    }

    /// Copy `from`'s author rows to `to`: a `keep_source` move, where both
    /// rows live on and each shows the copied conversation. Answers how many
    /// rows were copied.
    pub fn copy_prompt_authors(&self, from: i64, to: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "INSERT INTO prompt_authors (session_id, at, head, author, author_person_id) \
             SELECT ?2, at, head, author, author_person_id FROM prompt_authors \
              WHERE session_id = ?1 ORDER BY id",
            rusqlite::params![from, to],
        )?)
    }

    /// A session's author rows sent at or after `since`, oldest first.
    pub fn prompt_authors(
        &self,
        session_id: i64,
        since: i64,
    ) -> Result<Vec<PromptAuthorRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT at, head, author, author_person_id FROM prompt_authors \
              WHERE session_id = ?1 AND at >= ?2 ORDER BY at, id",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, since], |r| {
                Ok(PromptAuthorRow {
                    at: r.get(0)?,
                    head: r.get(1)?,
                    author: r.get(2)?,
                    author_person_id: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_folds_whitespace_and_is_cut() {
        assert_eq!(prompt_head("  fix\r\n the   test \n"), "fix the test");
        assert_eq!(
            prompt_head(&"é".repeat(200)).chars().count(),
            PROMPT_HEAD_CHARS
        );
        assert_eq!(prompt_head(" \n "), "");
    }

    #[test]
    fn rows_are_kept_per_session_and_read_since() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let sid = s
            .upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.record_prompt_author(sid, "first", "Ada · phone", Some(2), 10)
            .unwrap();
        s.record_prompt_author(sid, "  ", "Ada · phone", Some(2), 11)
            .unwrap();
        s.record_prompt_author(sid, "second\nline", "Bo · desktop", None, 20)
            .unwrap();
        let all = s.prompt_authors(sid, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].head, "second line");
        assert_eq!(all[0].author_person_id, Some(2));
        assert_eq!(s.prompt_authors(sid, 15).unwrap().len(), 1);
        for i in 0..KEEP_PER_SESSION + 5 {
            s.record_prompt_author(sid, "x", "Ada", None, 100 + i)
                .unwrap();
        }
        assert_eq!(
            s.prompt_authors(sid, 0).unwrap().len() as i64,
            KEEP_PER_SESSION
        );
    }

    #[test]
    fn a_cancelled_queued_prompt_forgets_its_row_and_only_it() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let sid = s
            .upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.record_prompt_author(sid, "run the tests", "Ada · phone", Some(2), 5)
            .unwrap();
        s.record_prompt_author(sid, "run the tests", "Ada · phone", Some(2), 101)
            .unwrap();
        s.record_prompt_author(sid, "other", "Ada · phone", Some(2), 101)
            .unwrap();
        // Queued at 100: the row written at 101 goes, the older one stays.
        assert!(s.forget_prompt_author(sid, "run  the\ntests", 100).unwrap());
        let left: Vec<_> = s
            .prompt_authors(sid, 0)
            .unwrap()
            .into_iter()
            .map(|r| (r.at, r.head))
            .collect();
        assert_eq!(
            left,
            vec![(5, "run the tests".into()), (101, "other".into())]
        );
        assert!(!s.forget_prompt_author(sid, "run the tests", 100).unwrap());
    }

    #[test]
    fn authors_move_or_are_copied_with_their_session() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let from = s
            .upsert_session("api-1", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let to = s
            .upsert_session("api-2", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let fork = s
            .upsert_session("api-3", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.record_prompt_author(from, "fix it", "Ada · phone", Some(2), 10)
            .unwrap();
        s.record_prompt_author(from, "queued", "Ada · phone", Some(2), 100)
            .unwrap();
        // keep_source: both rows keep them.
        assert_eq!(s.copy_prompt_authors(from, fork).unwrap(), 2);
        assert_eq!(
            s.prompt_authors(fork, 0).unwrap(),
            s.prompt_authors(from, 0).unwrap()
        );
        // A move: they follow, and survive the old row's delete.
        assert_eq!(s.repoint_prompt_authors(from, to).unwrap(), 2);
        s.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [from])
            .unwrap();
        let moved = s.prompt_authors(to, 0).unwrap();
        assert_eq!(moved.len(), 2);
        assert_eq!(moved[0].author, "Ada · phone");
        // A queued prompt cancelled after the move forgets its row there.
        assert!(s.forget_prompt_author(to, "queued", 100).unwrap());
    }
}
