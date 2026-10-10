//! Task comments (migration 161): a note on a task, kept in fleet and never
//! written to a tracker. About the ITEM, so the caller fences it with the
//! item; deleting one is its author's alone ([`Store::delete_comment`]).

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// Longest comment, in characters.
pub const COMMENT_MAX_CHARS: usize = 4000;
/// Most comments one task answers with: the newest.
pub const COMMENTS_SERVED_MAX: usize = 200;

/// One comment on a task, as `work { task }` serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRow {
    pub id: i64,
    pub item_id: i64,
    /// Who wrote it, as the hub names a caller (`client:<name>`,
    /// `host:<alias>`, `master`, `desktop`).
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<i64>,
    /// Plain text: rendered as text, never as markup.
    pub body: String,
    pub created_at: i64,
    /// The reader wrote it, so may delete it. Set by the reader's side
    /// ([`CommentRow::mark_mine`]); the store never knows who is asking.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
}

impl CommentRow {
    /// Whether `person` / `label` wrote it: the person when both sides
    /// prove one, else the caller's label.
    pub fn written_by(&self, person: Option<i64>, label: &str) -> bool {
        match (self.author_person_id, person) {
            (Some(a), Some(p)) => a == p,
            _ => self.author == label,
        }
    }

    pub fn mark_mine(&mut self, person: Option<i64>, label: &str) {
        self.mine = self.written_by(person, label);
    }
}

/// A typed comment, trimmed: not empty, at most [`COMMENT_MAX_CHARS`].
pub fn validate_comment(raw: &str) -> Result<String, IpcError> {
    let body = raw.trim();
    if body.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "a comment needs some text"));
    }
    if body.chars().count() > COMMENT_MAX_CHARS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a comment is at most {COMMENT_MAX_CHARS} characters"),
        ));
    }
    Ok(body.to_string())
}

fn map_comment(r: &rusqlite::Row<'_>) -> rusqlite::Result<CommentRow> {
    Ok(CommentRow {
        id: r.get(0)?,
        item_id: r.get(1)?,
        author: r.get(2)?,
        author_person_id: r.get(3)?,
        body: r.get(4)?,
        created_at: r.get(5)?,
        mine: false,
    })
}

const COLUMNS: &str = "id, item_id, author, author_person_id, body, created_at";

impl Store {
    /// Add a comment to work item `item_id`; emits the item, so every view
    /// of it re-reads.
    pub fn add_comment(
        &self,
        item_id: i64,
        author: &str,
        author_person_id: Option<i64>,
        body: &str,
    ) -> Result<CommentRow, IpcError> {
        let body = validate_comment(body)?;
        if self.get_work_item(item_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {item_id} not found"),
            ));
        }
        self.conn.execute(
            "INSERT INTO work_item_comments (item_id, author, author_person_id, body, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![item_id, author, author_person_id, body, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.emit_work_item(item_id, Default::default())?;
        self.get_comment(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "comment vanished after insert"))
    }

    /// One live comment.
    pub fn get_comment(&self, id: i64) -> Result<Option<CommentRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM work_item_comments WHERE id = ?1 AND deleted_at IS NULL"
                ),
                [id],
                map_comment,
            )
            .optional()?)
    }

    /// A task's live comments, oldest first: the newest
    /// [`COMMENTS_SERVED_MAX`].
    pub fn item_comments(&self, item_id: i64) -> Result<Vec<CommentRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM (SELECT * FROM work_item_comments \
               WHERE item_id = ?1 AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT ?2) \
             ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![item_id, COMMENTS_SERVED_MAX as i64],
            map_comment,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete comment `id`: its author's alone (`E_FORBIDDEN` for anyone
    /// else). `None` when it is unknown or already deleted.
    pub fn delete_comment(
        &self,
        id: i64,
        person: Option<i64>,
        label: &str,
    ) -> Result<Option<CommentRow>, IpcError> {
        let Some(c) = self.get_comment(id)? else {
            return Ok(None);
        };
        if !c.written_by(person, label) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "only its author deletes a comment",
            ));
        }
        self.conn.execute(
            "UPDATE work_item_comments SET deleted_at = ?1 WHERE id = ?2",
            rusqlite::params![now_unix(), id],
        )?;
        self.emit_work_item(c.item_id, Default::default())?;
        Ok(Some(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NativeItem, Store};

    fn item(s: &Store) -> i64 {
        s.create_native_item(&NativeItem {
            title: "Fix login",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap()
        .id
    }

    #[test]
    fn a_comment_is_kept_trimmed_and_served_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        s.add_comment(id, "desktop", None, "  first  ").unwrap();
        s.add_comment(id, "client:phone", Some(7), "second")
            .unwrap();
        let all = s.item_comments(id).unwrap();
        assert_eq!(
            all.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(all[1].author_person_id, Some(7));
        assert_eq!(
            s.add_comment(id, "desktop", None, "  ").unwrap_err().code,
            codes::E_INVALID
        );
        let long = "x".repeat(COMMENT_MAX_CHARS + 1);
        assert_eq!(
            s.add_comment(id, "desktop", None, &long).unwrap_err().code,
            codes::E_INVALID
        );
        assert_eq!(
            s.add_comment(999_999, "desktop", None, "x")
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn only_its_author_deletes_a_comment_and_it_is_not_served_again() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let by_person = s.add_comment(id, "client:phone", Some(7), "mine").unwrap();
        let by_label = s.add_comment(id, "desktop", None, "the desktop's").unwrap();

        // Another person, even under the same label, and another label.
        let err = s
            .delete_comment(by_person.id, Some(8), "client:phone")
            .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        let err = s
            .delete_comment(by_label.id, None, "client:phone")
            .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);

        // The same person from another device; the label with no person.
        assert!(s
            .delete_comment(by_person.id, Some(7), "client:laptop")
            .unwrap()
            .is_some());
        assert!(s
            .delete_comment(by_label.id, None, "desktop")
            .unwrap()
            .is_some());
        assert!(s.item_comments(id).unwrap().is_empty());
        assert!(s
            .delete_comment(by_label.id, None, "desktop")
            .unwrap()
            .is_none());
    }

    #[test]
    fn mine_is_the_readers_comment() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let mut c = s.add_comment(id, "desktop", None, "x").unwrap();
        c.mark_mine(None, "desktop");
        assert!(c.mine);
        c.mark_mine(Some(3), "client:phone");
        assert!(!c.mine);
    }
}
