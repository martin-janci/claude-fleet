//! Work graph (migration 046): work items, and the links that say which work
//! a session is doing. See `docs/superpowers/specs/2026-09-24-work-graph-design.md`
//! §0 and the migration for the model; the rules that matter here:
//!
//! * A link is anchored on the session's PARTICIPANT, never on the reusable
//!   `sessions.id`, so a move (which re-points the participant) carries it.
//! * A link names an item OR a bare key (`ref_key`): work exists before any
//!   tracker knows it.
//! * A user's decision is final: `confirmed` makes the link primary,
//!   `rejected` is sticky (later detection must not re-propose it), and only
//!   a later explicit decision changes either.
//! * Nothing here deletes history: a session's links END (with a snapshot)
//!   when its participant retires — that is the migration's trigger — and
//!   only an explicit unlink removes a live one.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// Why a link exists, as far as this slice can set it. Detection sources
/// (branch, pr, url, prompt …) arrive with roadmap M4.
pub const WORK_LINK_SOURCES: &[&str] = &["manual", "started", "agent"];

/// Longest accepted work key / free-form work reference.
pub const WORK_REF_MAX_CHARS: usize = 64;

/// Longest accepted local work item title.
pub const WORK_TITLE_MAX_CHARS: usize = 200;

/// A unit of work fleet knows by more than a key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkItemRow {
    pub id: i64,
    /// `local` today; a tracker provider's name once M3 lands.
    pub source: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    /// `todo` | `in_progress` | `done` (tracker items; a local item stays
    /// `todo` until someone says otherwise).
    #[serde(default)]
    pub status_category: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One session ↔ work link. `participant_id` is `None` once the retired
/// participant was swept; `ended_at` says the session is gone and `snap_*`
/// say what it was.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkLinkRow {
    pub id: i64,
    #[serde(default)]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub ref_key: Option<String>,
    #[serde(default)]
    pub participant_id: Option<i64>,
    /// `confirmed` | `rejected`.
    pub state: String,
    pub source: String,
    #[serde(default)]
    pub is_primary: bool,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub snap_host: Option<String>,
    #[serde(default)]
    pub snap_tmux: Option<String>,
    #[serde(default)]
    pub snap_name: Option<String>,
    #[serde(default)]
    pub snap_project_id: Option<i64>,
    #[serde(default)]
    pub snap_worktree: Option<String>,
    #[serde(default)]
    pub snap_branch: Option<String>,
    #[serde(default)]
    pub snap_pr_url: Option<String>,
    /// JSON array of every Claude conversation id the session ran.
    #[serde(default)]
    pub snap_claude_ids: Option<String>,
}

/// A live session's primary work, for the session row and the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkSummary {
    pub link_id: i64,
    #[serde(default)]
    pub item_id: Option<i64>,
    /// The item's key, else the link's `ref_key`.
    #[serde(default)]
    pub key: Option<String>,
    /// The item's title (empty for a bare key).
    #[serde(default)]
    pub title: String,
    pub source: String,
}

/// What to link a session to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkTarget<'a> {
    /// An existing work item.
    Item(i64),
    /// A key or free-form work reference (normalised by [`normalize_work_ref`]).
    Key(&'a str),
}

/// Normalise a work key / free-form work reference.
///
/// A ticket-shaped key (`abc-123`, `ENG2-7`) is upper-cased, so the same
/// ticket typed two ways is one link; anything else ("billing migration") is
/// kept as trimmed. Refused: empty, longer than [`WORK_REF_MAX_CHARS`], or
/// containing a control character.
pub fn normalize_work_ref(raw: &str) -> Result<String, IpcError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work key must not be empty",
        ));
    }
    if t.chars().count() > WORK_REF_MAX_CHARS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("work key longer than {WORK_REF_MAX_CHARS} characters"),
        ));
    }
    if t.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work key must not contain control characters",
        ));
    }
    Ok(if is_ticket_key(t) {
        t.to_ascii_uppercase()
    } else {
        t.to_string()
    })
}

/// `PREFIX-123`: a letter, then 1–9 of `[A-Za-z0-9_]`, a dash, 1–7 digits —
/// the shape the frontend's `extractWorkKey` recognises.
fn is_ticket_key(s: &str) -> bool {
    let Some((prefix, num)) = s.split_once('-') else {
        return false;
    };
    let mut chars = prefix.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    first_ok
        && (2..=10).contains(&prefix.len())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && (1..=7).contains(&num.len())
        && num.chars().all(|c| c.is_ascii_digit())
}

const LINK_COLUMNS: &str = "id, item_id, ref_key, participant_id, state, source, is_primary, \
     created_at, decided_at, ended_at, snap_host, snap_tmux, snap_name, snap_project_id, \
     snap_worktree, snap_branch, snap_pr_url, snap_claude_ids";

fn map_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkLinkRow> {
    Ok(WorkLinkRow {
        id: r.get(0)?,
        item_id: r.get(1)?,
        ref_key: r.get(2)?,
        participant_id: r.get(3)?,
        state: r.get(4)?,
        source: r.get(5)?,
        is_primary: r.get::<_, i64>(6)? != 0,
        created_at: r.get(7)?,
        decided_at: r.get(8)?,
        ended_at: r.get(9)?,
        snap_host: r.get(10)?,
        snap_tmux: r.get(11)?,
        snap_name: r.get(12)?,
        snap_project_id: r.get(13)?,
        snap_worktree: r.get(14)?,
        snap_branch: r.get(15)?,
        snap_pr_url: r.get(16)?,
        snap_claude_ids: r.get(17)?,
    })
}

const ITEM_COLUMNS: &str = "id, source, key, title, url, status_category, created_at, updated_at";

fn map_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkItemRow> {
    Ok(WorkItemRow {
        id: r.get(0)?,
        source: r.get(1)?,
        key: r.get(2)?,
        title: r.get(3)?,
        url: r.get(4)?,
        status_category: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

impl Store {
    /// Create a local work item, or return the local item that already has
    /// `key` (updating its title when a non-empty one is given). A local item
    /// with no key is always new.
    pub fn create_local_work_item(
        &self,
        key: Option<&str>,
        title: &str,
    ) -> Result<WorkItemRow, IpcError> {
        let key = key.map(normalize_work_ref).transpose()?;
        let title = title.trim();
        if title.chars().count() > WORK_TITLE_MAX_CHARS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("work title longer than {WORK_TITLE_MAX_CHARS} characters"),
            ));
        }
        if key.is_none() && title.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a work item needs a key or a title",
            ));
        }
        let now = now_unix();
        if let Some(k) = key.as_deref() {
            if let Some(existing) = self.local_work_item_by_key(k)? {
                if !title.is_empty() && title != existing.title {
                    self.conn.execute(
                        "UPDATE work_items SET title = ?1, updated_at = ?2 WHERE id = ?3",
                        rusqlite::params![title, now, existing.id],
                    )?;
                }
                return self.get_work_item(existing.id)?.ok_or_else(|| {
                    IpcError::new(codes::E_INTERNAL, "work item vanished after update")
                });
            }
        }
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, created_at, updated_at) \
             VALUES ('local', ?1, ?2, ?3, ?3)",
            rusqlite::params![key, title, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_work_item(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))
    }

    pub fn get_work_item(&self, id: i64) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM work_items WHERE id = ?1"),
                rusqlite::params![id],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    fn local_work_item_by_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items WHERE source = 'local' AND key = ?1"
                ),
                rusqlite::params![key],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Resolve a target to `(item_id, ref_key)`: an item must exist; a key
    /// that a local item carries links to that item, any other key stays a
    /// bare reference.
    fn resolve_work_target(
        &self,
        target: WorkTarget<'_>,
    ) -> Result<(Option<i64>, Option<String>), IpcError> {
        match target {
            WorkTarget::Item(id) => {
                if self.get_work_item(id)?.is_none() {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("work item {id} not found"),
                    ));
                }
                Ok((Some(id), None))
            }
            WorkTarget::Key(raw) => {
                let key = normalize_work_ref(raw)?;
                match self.local_work_item_by_key(&key)? {
                    Some(item) => Ok((Some(item.id), None)),
                    None => Ok((None, Some(key))),
                }
            }
        }
    }

    /// The live participant of `session_id`, minting one if it has none
    /// (rows from before migration 045). `E_NOTFOUND` for a session row that
    /// does not exist — an identity is never minted for a dead id.
    fn work_participant(&self, session_id: i64) -> Result<i64, IpcError> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .map_err(IpcError::from)?;
        if !exists {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
            ));
        }
        self.ensure_participant_for_session(session_id)
    }

    /// Write a decision about `target` for `session_id`: `confirmed` (the
    /// link becomes the session's primary work) or `rejected` (sticky, never
    /// primary). Idempotent per (session, target): re-deciding updates the
    /// one live link instead of adding another. Emits `session_updated`, so
    /// the row's `work` follows.
    fn decide_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        state: &str,
        source: &str,
    ) -> Result<WorkLinkRow, IpcError> {
        if !WORK_LINK_SOURCES.contains(&source) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown work link source {source:?}; one of {}",
                    WORK_LINK_SOURCES.join(", ")
                ),
            ));
        }
        let (item_id, ref_key) = self.resolve_work_target(target)?;
        let participant = self.work_participant(session_id)?;
        let now = now_unix();
        let primary = state == "confirmed";

        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM work_links \
                 WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND item_id IS ?2 AND ref_key IS ?3",
                rusqlite::params![participant, item_id, ref_key],
                |r| r.get(0),
            )
            .optional()?;
        if primary {
            self.conn.execute(
                "UPDATE work_links SET is_primary = 0 \
                 WHERE participant_id = ?1 AND ended_at IS NULL",
                rusqlite::params![participant],
            )?;
        }
        let id = match existing {
            Some(id) => {
                self.conn.execute(
                    "UPDATE work_links SET state = ?1, source = ?2, is_primary = ?3, \
                     decided_at = ?4 WHERE id = ?5",
                    rusqlite::params![state, source, primary as i64, now, id],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                             is_primary, created_at, decided_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    rusqlite::params![
                        item_id,
                        ref_key,
                        participant,
                        state,
                        source,
                        primary as i64,
                        now
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        self.emit_session(session_id)?;
        self.get_work_link(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished after write"))
    }

    /// Say that `session_id` works on `target`; it becomes the session's
    /// primary work. `source`: `manual` (a person), `started` (the session
    /// was created for it), `agent` (the in-session agent declared it).
    pub fn link_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
        source: &str,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work(session_id, target, "confirmed", source)
    }

    /// Say that `session_id` does NOT work on `target`. Sticky: detection
    /// must never re-propose it; only a later explicit link overrides it.
    pub fn reject_session_work(
        &self,
        session_id: i64,
        target: WorkTarget<'_>,
    ) -> Result<WorkLinkRow, IpcError> {
        self.decide_session_work(session_id, target, "rejected", "manual")
    }

    /// Remove one live link of `session_id` (a mistaken link, not a
    /// rejection: the target may be proposed again). `false` when the link
    /// does not exist, is not this session's, or has already ended — ended
    /// links are history and are never removed here.
    pub fn unlink_session_work(&self, session_id: i64, link_id: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM work_links WHERE id = ?1 AND ended_at IS NULL AND participant_id = \
               (SELECT id FROM participants WHERE session_id = ?2 AND retired_at IS NULL)",
            rusqlite::params![link_id, session_id],
        )?;
        if n > 0 {
            self.bump_session_for_work(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(n > 0)
    }

    /// A link write changes the row's `work` without touching `sessions`, so
    /// bump `row_version` by hand: the frontend's merge guard then orders the
    /// `session_updated` this emits after any older payload of the row.
    fn bump_session_for_work(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    pub fn get_work_link(&self, id: i64) -> Result<Option<WorkLinkRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {LINK_COLUMNS} FROM work_links WHERE id = ?1"),
                rusqlite::params![id],
                map_link,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Every live link of `session_id` (confirmed and rejected), primary
    /// first, then newest decision first.
    pub fn session_work_links(&self, session_id: i64) -> Result<Vec<WorkLinkRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM work_links \
             WHERE ended_at IS NULL AND participant_id = \
               (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL) \
             ORDER BY is_primary DESC, COALESCE(decided_at, created_at) DESC, id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![session_id], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Ended links to `key` (past work: its sessions are gone, their
    /// snapshots remain), newest first. Matches the link's own key or its
    /// item's key.
    pub fn ended_work_links_for_key(&self, key: &str) -> Result<Vec<WorkLinkRow>, IpcError> {
        let key = normalize_work_ref(key)?;
        let cols = LINK_COLUMNS
            .split(", ")
            .map(|c| format!("l.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols} FROM work_links l LEFT JOIN work_items i ON i.id = l.item_id \
             WHERE l.ended_at IS NOT NULL AND l.state = 'confirmed' \
               AND (l.ref_key = ?1 OR i.key = ?1) \
             ORDER BY l.ended_at DESC, l.id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![key], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// session id → its primary work, for every live session that has one.
    pub fn primary_work_by_session(&self) -> Result<HashMap<i64, WorkSummary>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.session_id, l.id, l.item_id, COALESCE(i.key, l.ref_key), \
                    COALESCE(i.title, ''), l.source \
             FROM work_links l \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN work_items i ON i.id = l.item_id \
             WHERE l.ended_at IS NULL AND l.is_primary = 1 AND l.state = 'confirmed' \
               AND p.session_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                WorkSummary {
                    link_id: r.get(1)?,
                    item_id: r.get(2)?,
                    key: r.get(3)?,
                    title: r.get(4)?,
                    source: r.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("h").unwrap();
        s.upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap()
    }

    #[test]
    fn normalizes_ticket_keys_and_keeps_free_form_refs() {
        assert_eq!(normalize_work_ref(" abc-123 ").unwrap(), "ABC-123");
        assert_eq!(normalize_work_ref("ENG2-7").unwrap(), "ENG2-7");
        assert_eq!(
            normalize_work_ref("billing migration").unwrap(),
            "billing migration"
        );
        let long = "x".repeat(WORK_REF_MAX_CHARS + 1);
        for bad in ["", "   ", "a\u{7}b", long.as_str()] {
            let err = normalize_work_ref(bad).unwrap_err();
            assert_eq!(err.code, codes::E_INVALID, "{bad:?}");
        }
    }

    #[test]
    fn linking_makes_the_newest_decision_primary() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let a = s
            .link_session_work(sid, WorkTarget::Key("abc-1"), "manual")
            .unwrap();
        assert_eq!(a.ref_key.as_deref(), Some("ABC-1"));
        assert!(a.is_primary);
        assert_eq!(a.state, "confirmed");

        let b = s
            .link_session_work(sid, WorkTarget::Key("DEF-2"), "agent")
            .unwrap();
        let links = s.session_work_links(sid).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].id, b.id, "primary first");
        assert!(links[0].is_primary);
        assert!(!links[1].is_primary);

        // Re-linking the first target updates its one link, not a second.
        let again = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert_eq!(again.id, a.id);
        assert_eq!(s.session_work_links(sid).unwrap().len(), 2);
        let summary = s.primary_work_by_session().unwrap();
        assert_eq!(summary[&sid].key.as_deref(), Some("ABC-1"));
    }

    #[test]
    fn a_rejection_is_sticky_and_never_primary() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let r = s
            .reject_session_work(sid, WorkTarget::Key("abc-1"))
            .unwrap();
        assert_eq!(r.state, "rejected");
        assert!(!r.is_primary);
        assert!(s.primary_work_by_session().unwrap().is_empty());
        // Still listed, so the UI can show and undo it.
        assert_eq!(s.session_work_links(sid).unwrap()[0].state, "rejected");
        // Only an explicit link overrides it.
        let back = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert_eq!(back.id, r.id);
        assert_eq!(back.state, "confirmed");
    }

    #[test]
    fn a_local_item_owns_its_key() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let item = s
            .create_local_work_item(Some("pay-7"), "Retry payments")
            .unwrap();
        assert_eq!(item.key.as_deref(), Some("PAY-7"));
        assert_eq!(
            s.create_local_work_item(Some("PAY-7"), "").unwrap().id,
            item.id,
            "one local item per key"
        );
        let renamed = s.create_local_work_item(Some("PAY-7"), "Retry v2").unwrap();
        assert_eq!(renamed.title, "Retry v2");
        // A key a local item carries links to the item.
        let l = s
            .link_session_work(sid, WorkTarget::Key("pay-7"), "manual")
            .unwrap();
        assert_eq!(l.item_id, Some(item.id));
        assert_eq!(l.ref_key, None);
        let summary = &s.primary_work_by_session().unwrap()[&sid];
        assert_eq!(summary.title, "Retry v2");
        assert_eq!(summary.key.as_deref(), Some("PAY-7"));
        // Keyless items are fine with a title; nothing at all is not.
        assert!(s.create_local_work_item(None, "Spike: search").is_ok());
        assert_eq!(
            s.create_local_work_item(None, " ").unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn unknown_sessions_items_and_sources_are_refused() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev");
        let err = s
            .link_session_work(4242, WorkTarget::Key("ABC-1"), "manual")
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(
            s.participant_for_session(4242).unwrap().is_none(),
            "no identity minted for a dead id"
        );
        let err = s
            .link_session_work(sid, WorkTarget::Item(99), "manual")
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let err = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "branch")
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[test]
    fn a_link_follows_the_session_through_a_move() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        let l = s
            .link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        // What move_session's finalise does: re-point the identity, then the
        // source row is deleted.
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].id, l.id);
        assert_eq!(links[0].ended_at, None, "a move does not end the work");
        assert_eq!(
            s.primary_work_by_session().unwrap()[&dst].key.as_deref(),
            Some("ABC-1")
        );
    }

    #[test]
    fn a_killed_sessions_link_ends_with_a_snapshot_instead_of_vanishing() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev-login");
        s.set_friendly_name("h", "dev-login", Some("Fix login"))
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(sid).unwrap();

        assert!(s.session_work_links(sid).unwrap().is_empty());
        assert!(s.primary_work_by_session().unwrap().is_empty());
        let past = s.ended_work_links_for_key("abc-1").unwrap();
        assert_eq!(past.len(), 1);
        let p = &past[0];
        assert!(p.ended_at.is_some());
        assert!(!p.is_primary);
        assert_eq!(p.snap_host.as_deref(), Some("h"));
        assert_eq!(p.snap_tmux.as_deref(), Some("dev-login"));
        assert_eq!(p.snap_name.as_deref(), Some("Fix login"));
    }

    #[test]
    fn unlink_removes_only_this_sessions_live_link() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let la = s
            .link_session_work(a, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        assert!(!s.unlink_session_work(b, la.id).unwrap(), "not b's link");
        assert!(s.unlink_session_work(a, la.id).unwrap());
        assert!(s.session_work_links(a).unwrap().is_empty());
        assert!(!s.unlink_session_work(a, la.id).unwrap(), "already gone");
    }

    #[test]
    fn the_session_row_carries_its_primary_work_and_every_change_is_emitted() {
        let (s, bus) = super::super::test_support::store_with_recorder();
        let sid = seed(&s, "dev");
        let before = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(before.work, None);
        bus.take();

        let item = s.create_local_work_item(Some("abc-9"), "Login").unwrap();
        let link = s
            .link_session_work(sid, WorkTarget::Key("ABC-9"), "manual")
            .unwrap();
        let row = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(
            row.work,
            Some(WorkSummary {
                link_id: link.id,
                item_id: Some(item.id),
                key: Some("ABC-9".into()),
                title: "Login".into(),
                source: "manual".into(),
            })
        );
        assert!(row.row_version > before.row_version);
        assert_eq!(bus.take(), vec![format!("session:updated:{sid}")]);

        assert!(row.work_rejected.is_empty());
        // A rejection of the primary leaves the row without work, and names
        // the key so a client's own recognition does not show it either.
        s.reject_session_work(sid, WorkTarget::Item(item.id))
            .unwrap();
        let row = s.get_session("dev", "h").unwrap().unwrap();
        assert_eq!(row.work, None);
        assert_eq!(row.work_rejected, vec!["ABC-9".to_string()]);
        assert_eq!(bus.take().len(), 1);

        let bare = s
            .link_session_work(sid, WorkTarget::Key("billing"), "agent")
            .unwrap();
        let w = s.get_session("dev", "h").unwrap().unwrap().work.unwrap();
        assert_eq!((w.key.as_deref(), w.title.as_str()), (Some("billing"), ""));
        bus.take();
        assert!(s.unlink_session_work(sid, bare.id).unwrap());
        assert_eq!(s.get_session("dev", "h").unwrap().unwrap().work, None);
        assert_eq!(bus.take().len(), 1);
        // A no-op unlink emits nothing.
        assert!(!s.unlink_session_work(sid, bare.id).unwrap());
        assert!(bus.take().is_empty());
    }
}
