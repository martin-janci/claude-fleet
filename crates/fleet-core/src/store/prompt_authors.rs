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
}
