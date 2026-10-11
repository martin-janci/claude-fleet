//! Task attachments (migration 166): files and images on a task, kept in
//! fleet and never written to a tracker. About the ITEM, as a comment is, so
//! the caller fences it with the item; deleting one is its author's alone
//! ([`Store::delete_attachment`]).
//!
//! The bytes are content-addressed: `work_attachment_blobs` holds each
//! distinct file once, keyed by its SHA-256 (computed here, never taken from
//! the caller), and the blob goes when the last live attachment naming it is
//! deleted. They live in SQLite so a hub backup carries them.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// The most `work.attachment_max_mb` may be set to, in MiB. Bounded by the
/// desktop↔hub transport, not by SQLite: a routed `attach` carries the file
/// as base64 (4/3 of it) in one `/mcp` request, and `work { attachment }`
/// answers it the same way, so the hub's request cap (`MCP_BODY_MAX`) and
/// the desktop's response cap (`http_client::MAX_RESPONSE`) are both sized
/// from [`ATTACHMENT_WIRE_BYTES`].
pub const ATTACHMENT_MAX_MB_CEILING: u64 = 16;
/// One attachment at the ceiling as it crosses the wire: its base64, plus
/// 1 MiB for the JSON-RPC and SSE framing around it.
pub const ATTACHMENT_WIRE_BYTES: usize =
    ((ATTACHMENT_MAX_MB_CEILING as usize * 1024 * 1024).div_ceil(3) * 4) + 1024 * 1024;

/// Longest attachment name, in characters.
pub const ATTACHMENT_NAME_MAX_CHARS: usize = 200;
/// Most attachments one task answers with: the newest.
pub const ATTACHMENTS_SERVED_MAX: usize = 200;
/// The type an attachment is stored as when neither the caller's type nor
/// its bytes name one on [`ATTACHMENT_MIMES`]: served for download, never
/// rendered.
pub const OCTET_STREAM: &str = "application/octet-stream";
/// The types an attachment is stored as. Anything else is stored as
/// [`OCTET_STREAM`] — except SVG, which is refused outright: it is markup
/// that can carry script.
pub const ATTACHMENT_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/pdf",
    "text/plain",
    "text/markdown",
    "text/csv",
    "application/json",
    "application/zip",
    "application/gzip",
    OCTET_STREAM,
];

/// One attachment on a task, as `work { task }` serves it: metadata only,
/// the bytes are `work { attachment }`'s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub id: i64,
    pub item_id: i64,
    pub name: String,
    pub mime: String,
    /// Bytes.
    pub size: i64,
    /// Hex SHA-256 of the bytes.
    pub sha256: String,
    /// Who added it, as the hub names a caller (`client:<name>`,
    /// `host:<alias>`, `master`, `desktop`); empty when withheld.
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<i64>,
    /// The comment it was added with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<i64>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub created_at: i64,
    /// The reader added it, so may delete it. Set by the reader's side
    /// ([`AttachmentRow::mark_mine`]); the store never knows who is asking.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
}

impl AttachmentRow {
    /// Whether `person` / `label` added it: the person when both sides
    /// prove one, else the caller's label (a comment's rule).
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

/// What a new attachment carries besides its bytes.
#[derive(Debug, Clone, Copy)]
pub struct NewAttachment<'a> {
    pub item_id: i64,
    pub name: &'a str,
    /// The caller's type; checked against the bytes for an image.
    pub mime: &'a str,
    pub author: &'a str,
    pub author_person_id: Option<i64>,
    pub comment_id: Option<i64>,
}

/// A file name, trimmed: not empty, at most [`ATTACHMENT_NAME_MAX_CHARS`],
/// no path separator, no control character, not `.` or `..`.
pub fn validate_attachment_name(raw: &str) -> Result<String, IpcError> {
    let name = raw.trim();
    let bad = |why: &str| IpcError::new(codes::E_INVALID, format!("an attachment name {why}"));
    if name.is_empty() {
        return Err(bad("cannot be empty"));
    }
    if name.chars().count() > ATTACHMENT_NAME_MAX_CHARS {
        return Err(bad(&format!(
            "is at most {ATTACHMENT_NAME_MAX_CHARS} characters"
        )));
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(bad("is a file name, not a path"));
    }
    if name.chars().any(char::is_control) {
        return Err(bad("cannot hold control characters"));
    }
    Ok(name.to_string())
}

/// The type the bytes' first bytes prove, for the types a signature names.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        Some("application/zip")
    } else if bytes.starts_with(&[0x1F, 0x8B]) {
        Some("application/gzip")
    } else {
        None
    }
}

/// The type an attachment is stored as: the caller's when it is on
/// [`ATTACHMENT_MIMES`] (an image's only when its bytes prove it), else
/// what the bytes prove, else [`OCTET_STREAM`]. SVG is refused.
pub fn attachment_mime(claimed: &str, bytes: &[u8]) -> Result<String, IpcError> {
    let claimed = claimed
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if claimed == "image/svg+xml" || claimed.contains("svg") {
        return Err(IpcError::new(
            codes::E_INVALID,
            "an SVG cannot be attached: it can carry script; attach a PNG of it",
        ));
    }
    let sniffed = sniff_mime(bytes);
    if claimed.starts_with("image/") {
        return match sniffed {
            Some(s) if s.starts_with("image/") => Ok(s.to_string()),
            _ => Err(IpcError::new(
                codes::E_INVALID,
                format!("the bytes are not a {claimed} image (PNG, JPEG, GIF or WebP)"),
            )),
        };
    }
    if ATTACHMENT_MIMES.contains(&claimed.as_str()) && claimed != OCTET_STREAM {
        return Ok(claimed);
    }
    Ok(sniffed.unwrap_or(OCTET_STREAM).to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

fn map_attachment(r: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRow> {
    Ok(AttachmentRow {
        id: r.get(0)?,
        item_id: r.get(1)?,
        name: r.get(2)?,
        mime: r.get(3)?,
        size: r.get(4)?,
        sha256: r.get(5)?,
        author: r.get(6)?,
        author_person_id: r.get(7)?,
        comment_id: r.get(8)?,
        source: r.get(9)?,
        external_id: r.get(10)?,
        created_at: r.get(11)?,
        mine: false,
    })
}

const COLUMNS: &str = "id, item_id, name, mime, size, sha256, author, author_person_id, \
                       comment_id, source, external_id, created_at";

impl Store {
    /// Attach `bytes` to work item `n.item_id`: at most `max_bytes`, under a
    /// valid name and an allowed type ([`attachment_mime`]). The same bytes
    /// twice share one blob. Emits the item, so every view of it re-reads.
    pub fn add_attachment(
        &self,
        n: &NewAttachment<'_>,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<AttachmentRow, IpcError> {
        let name = validate_attachment_name(n.name)?;
        if bytes.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "an attachment cannot be empty",
            ));
        }
        if bytes.len() > max_bytes {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{name} is {} bytes; an attachment is at most {max_bytes} bytes \
                     (work.attachment_max_mb)",
                    bytes.len()
                ),
            ));
        }
        let mime = attachment_mime(n.mime, bytes)?;
        if self.get_work_item(n.item_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {} not found", n.item_id),
            ));
        }
        if let Some(cid) = n.comment_id {
            if self
                .get_comment(cid)?
                .is_none_or(|c| c.item_id != n.item_id)
            {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("comment {cid} is not on work item {}", n.item_id),
                ));
            }
        }
        let sha = sha256_hex(bytes);
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO work_attachment_blobs (sha256, bytes, size, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![sha, bytes, bytes.len() as i64, now],
        )?;
        tx.execute(
            "INSERT INTO work_item_attachments \
               (item_id, name, mime, size, sha256, author, author_person_id, comment_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                n.item_id,
                name,
                mime,
                bytes.len() as i64,
                sha,
                n.author,
                n.author_person_id,
                n.comment_id,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        tx.commit()?;
        self.emit_work_item(n.item_id, Default::default())?;
        self.get_attachment(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "attachment vanished after insert"))
    }

    /// One live attachment's metadata.
    pub fn get_attachment(&self, id: i64) -> Result<Option<AttachmentRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM work_item_attachments \
                     WHERE id = ?1 AND deleted_at IS NULL"
                ),
                [id],
                map_attachment,
            )
            .optional()?)
    }

    /// One live attachment's bytes.
    pub fn attachment_bytes(&self, id: i64) -> Result<Option<Vec<u8>>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT b.bytes FROM work_item_attachments a \
                 JOIN work_attachment_blobs b ON b.sha256 = a.sha256 \
                 WHERE a.id = ?1 AND a.deleted_at IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// A task's live attachments, newest first: at most
    /// [`ATTACHMENTS_SERVED_MAX`].
    pub fn item_attachments(&self, item_id: i64) -> Result<Vec<AttachmentRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM work_item_attachments \
             WHERE item_id = ?1 AND deleted_at IS NULL \
             ORDER BY created_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![item_id, ATTACHMENTS_SERVED_MAX as i64],
            map_attachment,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete attachment `id`: its author's alone (`E_FORBIDDEN` for anyone
    /// else). The row stays, soft-deleted; its blob goes when no live
    /// attachment names it any more. `None` when it is unknown or already
    /// deleted.
    pub fn delete_attachment(
        &self,
        id: i64,
        person: Option<i64>,
        label: &str,
    ) -> Result<Option<AttachmentRow>, IpcError> {
        let Some(a) = self.get_attachment(id)? else {
            return Ok(None);
        };
        if !a.written_by(person, label) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "only its author deletes an attachment",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE work_item_attachments SET deleted_at = ?1 WHERE id = ?2",
            rusqlite::params![now_unix(), id],
        )?;
        tx.execute(
            "DELETE FROM work_attachment_blobs WHERE sha256 = ?1 AND NOT EXISTS \
               (SELECT 1 FROM work_item_attachments WHERE sha256 = ?1 AND deleted_at IS NULL)",
            [&a.sha256],
        )?;
        tx.commit()?;
        self.emit_work_item(a.item_id, Default::default())?;
        Ok(Some(a))
    }

    /// How many blobs the store holds (tests, and a backup's size check).
    pub fn attachment_blob_count(&self) -> Result<i64, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM work_attachment_blobs", [], |r| {
                r.get(0)
            })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NativeItem, Store};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const MAX: usize = 1024;

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

    fn new<'a>(item_id: i64, name: &'a str, mime: &'a str) -> NewAttachment<'a> {
        NewAttachment {
            item_id,
            name,
            mime,
            author: "desktop",
            author_person_id: None,
            comment_id: None,
        }
    }

    #[test]
    fn an_attachment_is_kept_with_its_digest_and_served_newest_first() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let a = s
            .add_attachment(&new(id, " shot.png ", "image/png"), PNG, MAX)
            .unwrap();
        assert_eq!(a.name, "shot.png");
        assert_eq!(a.mime, "image/png");
        assert_eq!(a.size, PNG.len() as i64);
        assert_eq!(a.sha256, sha256_hex(PNG));
        assert_eq!(a.source, "fleet");
        let b = s
            .add_attachment(&new(id, "notes.txt", "text/plain"), b"hello", MAX)
            .unwrap();
        let all = s.item_attachments(id).unwrap();
        assert_eq!(
            all.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        assert_eq!(s.attachment_bytes(a.id).unwrap().unwrap(), PNG);
        assert_eq!(s.attachment_bytes(b.id).unwrap().unwrap(), b"hello");
        assert_eq!(s.attachment_bytes(999).unwrap(), None);
    }

    #[test]
    fn the_same_bytes_share_one_blob_and_the_last_delete_drops_it() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let a = s
            .add_attachment(&new(id, "a.png", "image/png"), PNG, MAX)
            .unwrap();
        let b = s
            .add_attachment(&new(id, "b.png", "image/png"), PNG, MAX)
            .unwrap();
        assert_eq!(s.attachment_blob_count().unwrap(), 1);
        s.delete_attachment(a.id, None, "desktop").unwrap().unwrap();
        // b still names the blob.
        assert_eq!(s.attachment_blob_count().unwrap(), 1);
        assert_eq!(s.attachment_bytes(b.id).unwrap().unwrap(), PNG);
        assert_eq!(s.attachment_bytes(a.id).unwrap(), None);
        s.delete_attachment(b.id, None, "desktop").unwrap().unwrap();
        assert_eq!(s.attachment_blob_count().unwrap(), 0);
        assert!(s.item_attachments(id).unwrap().is_empty());
        assert!(s
            .delete_attachment(b.id, None, "desktop")
            .unwrap()
            .is_none());
        // The same file again after its blob went: stored afresh.
        let c = s
            .add_attachment(&new(id, "c.png", "image/png"), PNG, MAX)
            .unwrap();
        assert_eq!(s.attachment_bytes(c.id).unwrap().unwrap(), PNG);
    }

    #[test]
    fn only_its_author_deletes_an_attachment() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let mut n = new(id, "a.txt", "text/plain");
        n.author = "client:phone";
        n.author_person_id = Some(7);
        let by_person = s.add_attachment(&n, b"x", MAX).unwrap();
        let by_label = s
            .add_attachment(&new(id, "b.txt", "text/plain"), b"y", MAX)
            .unwrap();
        let forbidden =
            |r: Result<_, IpcError>| assert_eq!(r.unwrap_err().code, codes::E_FORBIDDEN);
        forbidden(s.delete_attachment(by_person.id, Some(8), "client:phone"));
        forbidden(s.delete_attachment(by_label.id, None, "client:phone"));
        assert!(s
            .delete_attachment(by_person.id, Some(7), "client:laptop")
            .unwrap()
            .is_some());
        assert!(s
            .delete_attachment(by_label.id, None, "desktop")
            .unwrap()
            .is_some());
        let mut c = s
            .add_attachment(&new(id, "c.txt", "text/plain"), b"z", MAX)
            .unwrap();
        c.mark_mine(None, "desktop");
        assert!(c.mine);
        c.mark_mine(Some(3), "client:phone");
        assert!(!c.mine);
    }

    #[test]
    fn limits_names_and_types_are_checked_before_anything_is_stored() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let invalid = |n: NewAttachment<'_>, bytes: &[u8]| {
            assert_eq!(
                s.add_attachment(&n, bytes, MAX).unwrap_err().code,
                codes::E_INVALID,
                "{n:?}"
            )
        };
        invalid(new(id, "big.bin", OCTET_STREAM), &vec![0u8; MAX + 1]);
        invalid(new(id, "empty.txt", "text/plain"), b"");
        invalid(new(id, "  ", "text/plain"), b"x");
        invalid(new(id, "../etc/passwd", "text/plain"), b"x");
        invalid(new(id, "a\\b.txt", "text/plain"), b"x");
        invalid(new(id, "bell\u{7}.txt", "text/plain"), b"x");
        let long = "x".repeat(ATTACHMENT_NAME_MAX_CHARS + 1);
        invalid(new(id, &long, "text/plain"), b"x");
        // SVG is markup that can carry script: refused, whatever it holds.
        invalid(
            new(id, "logo.svg", "image/svg+xml"),
            b"<svg onload=alert(1)>",
        );
        invalid(
            new(id, "logo.svg", "Image/SVG+XML; charset=utf-8"),
            b"<svg/>",
        );
        // An image whose bytes are not one.
        invalid(new(id, "x.png", "image/png"), b"<html><script>");
        assert_eq!(s.attachment_blob_count().unwrap(), 0);
        assert_eq!(
            s.add_attachment(&new(999_999, "a.txt", "text/plain"), b"x", MAX)
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );
        // Exactly the limit is fine.
        assert!(s
            .add_attachment(&new(id, "max.bin", OCTET_STREAM), &vec![1u8; MAX], MAX)
            .is_ok());
    }

    #[test]
    fn an_unlisted_type_is_stored_as_what_the_bytes_prove_or_octet_stream() {
        assert_eq!(
            attachment_mime("text/html", b"<html>").unwrap(),
            OCTET_STREAM
        );
        assert_eq!(attachment_mime("", b"%PDF-1.7").unwrap(), "application/pdf");
        assert_eq!(attachment_mime("", PNG).unwrap(), "image/png");
        assert_eq!(
            attachment_mime("text/markdown; charset=utf-8", b"# hi").unwrap(),
            "text/markdown"
        );
        // An image type follows its bytes: a JPEG sent as PNG is a JPEG.
        assert_eq!(
            attachment_mime("image/png", &[0xFF, 0xD8, 0xFF, 0xE0]).unwrap(),
            "image/jpeg"
        );
    }

    #[test]
    fn a_comment_link_must_be_a_comment_on_the_same_item() {
        let s = Store::open_in_memory().unwrap();
        let id = item(&s);
        let other = item(&s);
        let c = s
            .add_comment(id, "desktop", None, "see the screenshot")
            .unwrap();
        let mut n = new(id, "a.png", "image/png");
        n.comment_id = Some(c.id);
        assert_eq!(
            s.add_attachment(&n, PNG, MAX).unwrap().comment_id,
            Some(c.id)
        );
        n.item_id = other;
        assert_eq!(
            s.add_attachment(&n, PNG, MAX).unwrap_err().code,
            codes::E_INVALID
        );
    }
}
