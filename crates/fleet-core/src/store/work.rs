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
    // --- tracker attributes (migration 048, work graph M3); all default, so
    // an older hub's row still reads. Identity is (tracker_id, external_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Former keys (a moved or renamed issue), upper case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// The tracker's type name (Story, Bug, Epic …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Jira `issuetype.hierarchyLevel`: 1 epic, 0 standard, -1 subtask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy_level: Option<i64>,
    /// The tracker's own status name ("In Review").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    /// completed | not_planned | duplicate, once resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    /// Display names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    /// The current sprint's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<String>,
    /// The tracker's own `updated`, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_ext: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_changed_at: Option<i64>,
    /// When fleet last read it from the tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// Missing is not gone (C25): the tracker stopped answering for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_at: Option<i64>,
    /// not_found_or_no_permission | tracker_removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
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
    /// `work` | `review` | `worker` (a link inherited from a parent).
    #[serde(default = "default_role")]
    pub role: String,
    /// `false` once `purge_project` removed the transcripts the link's
    /// conversations would resume from.
    #[serde(default = "default_true")]
    pub resumable: bool,
    // --- detection (migration 049, work graph M4); all default, so an older
    // hub's row still reads.
    /// The conversation the link was decided in (a suggestion: last seen
    /// in); `None` for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// explicit | strong | weak; `None` for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    /// The resolver rule that made it (R3, R5 …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Why: what was seen, oldest first (`service::work::resolve::Evidence`
    /// objects: signal, rule, text, snippet?, at, conversation?, note?).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<serde_json::Value>,
    /// A suggestion shown pre-selected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preselected: bool,
    /// Why a live session's link ended (`branch_changed`, `pr_changed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    /// The link's org (work graph M5): its tracker item's, else its
    /// session's (live) or the session's org when it ended (past work).
    /// Filled by [`Store::fill_link_orgs`]; `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

fn default_role() -> String {
    "work".into()
}

fn default_true() -> bool {
    true
}

/// A live session's primary work, for the session row and the sidebar.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    // --- the tracker item's status (work graph M3), absent for a bare key,
    // a local item, or a hub older than M3.
    /// todo | in_progress | done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_category: Option<String>,
    /// The tracker's own status name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The tracker no longer answers for the item (C25).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unavailable: bool,
    // --- explanation (work graph M4); absent from an older hub.
    /// confirmed | suggested.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// explicit | strong | weak; absent for links older than M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    /// The resolver rule that made it (R3, R5 …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// A suggestion shown pre-selected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preselected: bool,
    /// Live link suggestions the session has, still to decide.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub suggestions: u32,
    /// The link's org (work graph M5): its tracker's, else the session's.
    /// `None` = unassigned. Absent from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    // --- lifecycle (work graph M7); absent from an older hub.
    /// The live session was archived from the UI (collapsed into its
    /// group's Done; tmux keeps running) at this unix second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// What to link a session to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkTarget<'a> {
    /// An existing work item.
    Item(i64),
    /// A key or free-form work reference (normalised by [`normalize_work_ref`]).
    Key(&'a str),
    /// A key linked as a bare reference, never resolved to an item (work
    /// graph M5): what a per-host token gets when it names a key another
    /// org's tracker owns — exactly what an unknown key gives it, so the
    /// answer says nothing about the other org.
    Ref(&'a str),
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
    Ok(canonical_key(t))
}

/// The one spelling of a reference, as items and links store it: a ticket
/// key upper-cased (`ABC-12`), a GitHub `owner/repo#n` lower-cased, anything
/// else (`asana:<gid>`, free text) as trimmed.
pub fn canonical_key(raw: &str) -> String {
    let t = raw.trim();
    if is_ticket_key(t) {
        t.to_ascii_uppercase()
    } else if github_ref(t).is_some() {
        t.to_ascii_lowercase()
    } else {
        t.to_string()
    }
}

/// `owner/repo#n` → `(owner/repo, n)`, both names `[A-Za-z0-9_.-]`.
pub fn github_ref(s: &str) -> Option<(&str, u64)> {
    let (repo, n) = s.rsplit_once('#')?;
    let (o, r) = repo.split_once('/')?;
    let name_ok = |x: &str| {
        !x.is_empty()
            && !x.starts_with(['.', '-'])
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    if !(name_ok(o) && name_ok(r)) || n.is_empty() || n.len() > 9 {
        return None;
    }
    Some((repo, n.parse().ok()?))
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
     snap_worktree, snap_branch, snap_pr_url, snap_claude_ids, role, resumable, \
     claude_session_id, strength, rule, evidence, preselected, end_reason, snap_org_id";

/// Columns of [`LINK_COLUMNS`] (a join's `l.` prefix is added by callers).
pub(super) const LINK_COLUMN_COUNT: usize = 27;

/// [`LINK_COLUMNS`] with every column prefixed by `alias.`.
pub(super) fn link_columns_prefixed(alias: &str) -> String {
    LINK_COLUMNS
        .split(", ")
        .map(|c| format!("{alias}.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn map_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkLinkRow> {
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
        role: r.get(18)?,
        resumable: r.get::<_, i64>(19)? != 0,
        claude_session_id: r.get(20)?,
        strength: r.get(21)?,
        rule: r.get(22)?,
        evidence: r
            .get::<_, Option<String>>(23)?
            .and_then(|e| serde_json::from_str(&e).ok())
            .unwrap_or_default(),
        preselected: r.get::<_, i64>(24)? != 0,
        end_reason: r.get(25)?,
        // The snapshot's org; `Store::fill_link_orgs` resolves the rest.
        org_id: r.get(26)?,
    })
}

pub(super) const ITEM_COLUMNS: &str =
    "id, source, key, title, url, status_category, created_at, updated_at, \
     tracker_id, external_id, aliases, kind, hierarchy_level, status_name, resolution, parent_id, \
     assignees, iteration, updated_ext, status_changed_at, fetched_at, unavailable_at, \
     unavailable_reason";

/// A JSON array column as a list; anything unreadable is empty.
fn json_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(super) fn map_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkItemRow> {
    Ok(WorkItemRow {
        id: r.get(0)?,
        source: r.get(1)?,
        key: r.get(2)?,
        title: r.get(3)?,
        url: r.get(4)?,
        status_category: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        tracker_id: r.get(8)?,
        external_id: r.get(9)?,
        aliases: json_list(r.get(10)?),
        kind: r.get(11)?,
        hierarchy_level: r.get(12)?,
        status_name: r.get(13)?,
        resolution: r.get(14)?,
        parent_id: r.get(15)?,
        assignees: json_list(r.get(16)?),
        iteration: r.get(17)?,
        updated_ext: r.get(18)?,
        status_changed_at: r.get(19)?,
        fetched_at: r.get(20)?,
        unavailable_at: r.get(21)?,
        unavailable_reason: r.get(22)?,
    })
}

impl Store {
    /// Create a local work item, or return the local item that already has
    /// `key` (updating its title when a non-empty one is given). A local item
    /// with no key is always new.
    /// Keyed local work items changed since `since` (unix seconds), newest
    /// first — the classification nudge's (work graph M4.6) local
    /// candidates. A keyless item cannot be named back by key, so it is not
    /// one.
    pub fn recent_local_work_items(
        &self,
        since: i64,
        limit: usize,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
             WHERE source = 'local' AND key IS NOT NULL AND updated_at >= ?1 \
             ORDER BY updated_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit as i64], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

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

    /// Name a piece of work (roadmap M1, "Name this work…"): create the local
    /// item with `title` (and `key`, when given), or retitle the local item
    /// that has `key`. Links that name `key` as a bare reference bind to the
    /// item, and every session linked to it is re-emitted, so each row's work
    /// shows the title at once. A key a tracker already has is refused
    /// (`E_INVALID_STATE`): its title is the tracker's.
    pub fn name_local_work(&self, key: Option<&str>, title: &str) -> Result<WorkItemRow, IpcError> {
        if title.trim().is_empty() {
            return Err(IpcError::new(codes::E_INVALID, "naming work needs a title"));
        }
        let key = key.map(normalize_work_ref).transpose()?;
        if let Some(k) = key.as_deref() {
            if self.tracker_item_for_key(k)?.is_some() {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("{k} is a tracker's ticket; its title comes from the tracker"),
                ));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let item = self.create_local_work_item(key.as_deref(), title)?;
        if let Some(k) = item.key.as_deref() {
            // A participant that already links the item keeps that one link.
            self.conn.execute(
                "UPDATE work_links SET item_id = ?1, ref_key = NULL \
                 WHERE ref_key = ?2 AND item_id IS NULL \
                   AND NOT (ended_at IS NULL AND participant_id IN \
                     (SELECT participant_id FROM work_links \
                      WHERE item_id = ?1 AND ended_at IS NULL))",
                rusqlite::params![item.id, k],
            )?;
        }
        let sessions: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.item_id = ?1 AND l.ended_at IS NULL AND p.session_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![item.id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for sid in &sessions {
            self.bump_session_for_work(*sid)?;
        }
        tx.commit()?;
        for sid in sessions {
            self.emit_session(sid)?;
        }
        Ok(item)
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
            WorkTarget::Key(raw) => self.resolve_work_key(&normalize_work_ref(raw)?),
            WorkTarget::Ref(raw) => Ok((None, Some(normalize_work_ref(raw)?))),
        }
    }

    /// `(item_id, ref_key)` for a normalised key: a tracker item exactly one
    /// tracker has (by key or alias) wins; else a local item; else a bare key
    /// a later sync binds. The key is kept on a tracker link too, for history
    /// and for a tracker that is later removed.
    pub(super) fn resolve_work_key(
        &self,
        key: &str,
    ) -> Result<(Option<i64>, Option<String>), IpcError> {
        if let Some(item) = self.tracker_item_for_key(key)? {
            return Ok((Some(item.id), Some(key.to_string())));
        }
        match self.local_work_item_by_key(key)? {
            Some(item) => Ok((Some(item.id), None)),
            None => Ok((None, Some(key.to_string()))),
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
                   AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2)) \
                 ORDER BY id LIMIT 1",
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
            // A decision over a suggestion keeps its evidence and rule, so
            // the link can still say what proposed it.
            Some(id) => {
                self.conn.execute(
                    "UPDATE work_links SET state = ?1, source = ?2, is_primary = ?3, \
                     decided_at = ?4, strength = 'explicit', preselected = 0, \
                     claude_session_id = COALESCE((SELECT claude_session_id FROM sessions \
                                                   WHERE id = ?6), claude_session_id) \
                     WHERE id = ?5",
                    rusqlite::params![state, source, primary as i64, now, id, session_id],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                             is_primary, created_at, decided_at, strength, \
                                             claude_session_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 'explicit', \
                             (SELECT claude_session_id FROM sessions WHERE id = ?8))",
                    rusqlite::params![
                        item_id,
                        ref_key,
                        participant,
                        state,
                        source,
                        primary as i64,
                        now,
                        session_id
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

    /// Give `participant` a live confirmed link to `(item_id, ref_key)`
    /// because of something fleet did (`resumed`, `forked`, `inherited`),
    /// unless it already has a live link to that target — a confirmed one
    /// means the work is carried already, a rejected one is sticky. It
    /// becomes primary only when the participant has no primary work yet.
    /// `true` when a link was written.
    fn carry_link(
        &self,
        participant: i64,
        item_id: Option<i64>,
        ref_key: Option<&str>,
        source: &str,
        role: &str,
    ) -> Result<bool, IpcError> {
        // A carry is a decision fleet makes for the person: it settles a
        // live suggestion of the same target rather than sitting beside it.
        // The same target is the same item however it was spelled (by id,
        // or by its key), as `decide_session_work` matches it.
        self.conn.execute(
            "DELETE FROM work_links WHERE participant_id = ?1 AND ended_at IS NULL \
               AND state = 'suggested' \
               AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2))",
            rusqlite::params![participant, item_id, ref_key],
        )?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL \
               AND ((item_id IS ?2 AND ref_key IS ?3) OR (?2 IS NOT NULL AND item_id = ?2)))",
            rusqlite::params![participant, item_id, ref_key],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(false);
        }
        let has_primary: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL AND is_primary = 1)",
            rusqlite::params![participant],
            |r| r.get(0),
        )?;
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, role, \
                                     is_primary, created_at, decided_at, strength) \
             VALUES (?1, ?2, ?3, 'confirmed', ?4, ?5, ?6, ?7, ?7, 'explicit')",
            rusqlite::params![
                item_id,
                ref_key,
                participant,
                source,
                role,
                (!has_primary) as i64,
                now
            ],
        )?;
        Ok(true)
    }

    /// Resume auto-carry (review C7): `session_id` now runs Claude
    /// conversation `claude_session_id`; every ENDED confirmed link whose
    /// snapshot names that conversation is carried onto the session with
    /// source `resumed`. Called by the rebind (the one writer of a row's
    /// conversation id) inside its transaction; the caller emits the row.
    /// Returns how many links were written.
    pub(crate) fn carry_resumed_work(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<usize, IpcError> {
        let targets: Vec<(Option<i64>, Option<String>, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT l.item_id, l.ref_key, l.role FROM work_links l, \
                        json_each(l.snap_claude_ids) j \
                 WHERE l.ended_at IS NOT NULL AND l.state = 'confirmed' \
                   AND l.snap_claude_ids IS NOT NULL AND j.value = ?1 \
                 GROUP BY l.item_id, l.ref_key ORDER BY MAX(l.ended_at) DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![claude_session_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if targets.is_empty() {
            return Ok(0);
        }
        let participant = self.ensure_participant_for_session(session_id)?;
        let mut n = 0;
        for (item_id, ref_key, role) in targets {
            if self.carry_link(participant, item_id, ref_key.as_deref(), "resumed", &role)? {
                n += 1;
            }
        }
        if n > 0 {
            self.bump_session_for_work(session_id)?;
        }
        Ok(n)
    }

    /// `move_session { keep_source }` forks the session: copy the source's
    /// live confirmed links onto the target with source `forked`. Emits the
    /// target's row when anything was copied.
    pub fn copy_work_links(&self, from_session: i64, to_session: i64) -> Result<usize, IpcError> {
        let links: Vec<WorkLinkRow> = self
            .session_work_links(from_session)?
            .into_iter()
            .filter(|l| l.state == "confirmed")
            .collect();
        if links.is_empty() {
            return Ok(0);
        }
        let participant = self.work_participant(to_session)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        // Primary first, so the fork's primary is the source's.
        for l in &links {
            if self.carry_link(
                participant,
                l.item_id,
                l.ref_key.as_deref(),
                "forked",
                &l.role,
            )? {
                n += 1;
            }
        }
        if n > 0 {
            self.bump_session_for_work(to_session)?;
        }
        tx.commit()?;
        if n > 0 {
            self.emit_session(to_session)?;
        }
        Ok(n)
    }

    /// A resume started `session_id` for work `key`: link it with source
    /// `resumed` (a no-op when the rebind's carry already did). Emits the row.
    pub fn link_resumed_work(&self, session_id: i64, key: &str) -> Result<bool, IpcError> {
        let (item_id, ref_key) = self.resolve_work_target(WorkTarget::Key(key))?;
        let participant = self.work_participant(session_id)?;
        let wrote = self.carry_link(participant, item_id, ref_key.as_deref(), "resumed", "work")?;
        if wrote {
            self.bump_session_for_work(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(wrote)
    }

    /// [`Self::inherit_work`] for a task worker (`dispatch_task`, a
    /// background agent's requester), emitting the worker's row when it
    /// gained a link. Not part of `set_parent_session_id`: a move also sets
    /// that column (to its source), and a move carries work its own way.
    pub fn inherit_worker_work(&self, worker: i64, requester: i64) -> Result<bool, IpcError> {
        let wrote = self.inherit_work(worker, requester, "worker")?;
        if wrote {
            self.emit_session(worker)?;
        }
        Ok(wrote)
    }

    /// A review session (`role` `review`) or a task worker (`worker`)
    /// inherits its parent's primary work, source `inherited` — only when it
    /// has no live link of its own. The caller emits the child's row.
    pub(crate) fn inherit_work(
        &self,
        child_session: i64,
        parent_session: i64,
        role: &str,
    ) -> Result<bool, IpcError> {
        let Some(primary) = self
            .session_work_links(parent_session)?
            .into_iter()
            .find(|l| l.is_primary && l.state == "confirmed")
        else {
            return Ok(false);
        };
        if self
            .session_work_links(child_session)?
            .iter()
            .any(|l| l.state != "suggested")
        {
            return Ok(false);
        }
        let participant = self.work_participant(child_session)?;
        let wrote = self.carry_link(
            participant,
            primary.item_id,
            primary.ref_key.as_deref(),
            "inherited",
            role,
        )?;
        if wrote {
            self.bump_session_for_work(child_session)?;
        }
        Ok(wrote)
    }

    /// Work keys whose links would lose resumable conversations when
    /// `project_id` is purged on `hosts`: ended links snapshotted there, and
    /// live links of that project's sessions there. Sorted, distinct.
    pub fn work_keys_for_purge(
        &self,
        project_id: i64,
        hosts: &[String],
    ) -> Result<Vec<String>, IpcError> {
        let hosts = serde_json::to_string(hosts)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT COALESCE(i.key, l.ref_key, i.title) AS k FROM work_links l \
             LEFT JOIN work_items i ON i.id = l.item_id \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.state = 'confirmed' AND l.resumable = 1 AND ( \
               (l.ended_at IS NOT NULL AND l.snap_project_id = ?1 \
                  AND l.snap_host IN (SELECT value FROM json_each(?2))) \
               OR (l.ended_at IS NULL AND s.project_id = ?1 \
                  AND s.host_alias IN (SELECT value FROM json_each(?2)))) \
               AND k IS NOT NULL \
             ORDER BY k",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id, hosts], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// After a purge of `project_id` on `hosts`: mark the ended links
    /// snapshotted there `resumable = 0` (their transcripts are gone), so
    /// *continue* is disabled with a reason and *fresh with brief* remains.
    /// Live links end with the project's sessions right after, and are
    /// marked by the same rule once they have.
    pub fn mark_purged_work_unresumable(
        &self,
        project_id: i64,
        hosts: &[String],
    ) -> Result<usize, IpcError> {
        let hosts = serde_json::to_string(hosts)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        Ok(self.conn.execute(
            "UPDATE work_links SET resumable = 0 \
             WHERE ended_at IS NOT NULL AND resumable = 1 AND snap_project_id = ?1 \
               AND snap_host IN (SELECT value FROM json_each(?2))",
            rusqlite::params![project_id, hosts],
        )?)
    }

    /// The work item that carries `key` (normalised): a tracker's item when
    /// one exists, else the local one. A removed tracker's rows (kept for
    /// the links that point at them) come last: they must not shadow the
    /// same site re-added, whose fresh row carries the live title and org.
    pub fn work_item_by_key(&self, key: &str) -> Result<Option<WorkItemRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items WHERE key = ?1 \
                     ORDER BY (tracker_id IS NOT NULL \
                               AND tracker_id NOT IN (SELECT id FROM trackers)) ASC, \
                              (source = 'local') ASC, id ASC LIMIT 1"
                ),
                rusqlite::params![key],
                map_item,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Live confirmed links to `key` with the session each is on, newest
    /// decision first.
    pub fn live_work_sessions_for_key(
        &self,
        key: &str,
    ) -> Result<Vec<(WorkLinkRow, super::SessionRow)>, IpcError> {
        let key = normalize_work_ref(key)?;
        let cols = LINK_COLUMNS
            .split(", ")
            .map(|c| format!("l.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let pairs: Vec<(WorkLinkRow, i64)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {cols}, p.session_id FROM work_links l \
                 LEFT JOIN work_items i ON i.id = l.item_id \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.ended_at IS NULL AND l.state = 'confirmed' \
                   AND p.session_id IS NOT NULL AND (l.ref_key = ?1 OR i.key = ?1) \
                 ORDER BY COALESCE(l.decided_at, l.created_at) DESC, l.id DESC"
            ))?;
            let rows = stmt.query_map(rusqlite::params![key], |r| {
                Ok((map_link(r)?, r.get::<_, i64>(LINK_COLUMN_COUNT)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(pairs.len());
        for (link, sid) in pairs {
            if let Some(row) = self.get_session_by_id(sid)? {
                out.push((link, row));
            }
        }
        Ok(out)
    }

    /// Every work item some confirmed link points at, live or ended (the
    /// Today view's "shipped" reads a done ticket only when it is work).
    pub fn linked_work_item_ids(&self) -> Result<std::collections::HashSet<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT item_id FROM work_links \
             WHERE item_id IS NOT NULL AND state = 'confirmed'",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Confirmed links that ended at or after `since` (past work, for the
    /// sidebar's past-only groups), newest first, at most `limit`.
    pub fn recent_ended_work_links(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<WorkLinkRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM work_links \
             WHERE ended_at IS NOT NULL AND ended_at >= ?1 AND state = 'confirmed' \
             ORDER BY ended_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![since, limit], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// session id → its primary work, for every live session that has one.
    pub fn primary_work_by_session(&self) -> Result<HashMap<i64, WorkSummary>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.session_id, l.id, l.item_id, COALESCE(i.key, l.ref_key), \
                    COALESCE(i.title, ''), l.source, \
                    CASE WHEN i.tracker_id IS NOT NULL THEN i.status_category END, \
                    i.status_name, i.url, i.unavailable_at IS NOT NULL \
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
                    status_category: r.get(6)?,
                    status_name: r.get(7)?,
                    url: r.get(8)?,
                    unavailable: r.get::<_, Option<bool>>(9)?.unwrap_or(false),
                    state: "confirmed".into(),
                    ..Default::default()
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
    fn recently_ended_links_are_listed_newest_first() {
        let s = Store::open_in_memory().unwrap();
        let a = seed(&s, "a");
        let b = seed(&s, "b");
        let live = seed(&s, "live");
        for (sid, key) in [(a, "ABC-1"), (b, "DEF-2"), (live, "GHI-3")] {
            s.link_session_work(sid, WorkTarget::Key(key), "manual")
                .unwrap();
        }
        s.delete_session(a).unwrap();
        s.delete_session(b).unwrap();
        let recent = s.recent_ended_work_links(0, 10).unwrap();
        assert_eq!(recent.len(), 2, "live links are not past work");
        assert!(recent.iter().all(|l| l.ended_at.is_some()));
        assert!(s
            .recent_ended_work_links(now_unix() + 60, 10)
            .unwrap()
            .is_empty());
        assert_eq!(s.recent_ended_work_links(0, 1).unwrap().len(), 1);
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
                state: "confirmed".into(),
                strength: Some("explicit".into()),
                ..Default::default()
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

    fn with_conversation(s: &Store, name: &str, claude: &str) -> i64 {
        let id = seed(s, name);
        s.rebind_conversation(id, claude, crate::store::StartSource::Startup, None, None)
            .unwrap();
        id
    }

    #[test]
    fn a_resumed_conversation_carries_its_ended_work() {
        let (s, bus) = super::super::test_support::store_with_recorder();
        let old = with_conversation(&s, "old", "c-1");
        s.link_session_work(old, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(old).unwrap();

        let new = with_conversation(&s, "new", "c-fresh");
        assert!(
            s.session_work_links(new).unwrap().is_empty(),
            "a fresh id carries nothing"
        );
        bus.take();
        s.rebind_conversation(new, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        let links = s.session_work_links(new).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (
                links[0].ref_key.as_deref(),
                links[0].source.as_str(),
                links[0].is_primary
            ),
            (Some("ABC-1"), "resumed", true)
        );
        let row = s.get_session_by_id(new).unwrap().unwrap();
        assert_eq!(row.work.unwrap().key.as_deref(), Some("ABC-1"));
        assert!(bus.take().contains(&format!("session:updated:{new}")));

        // Resuming again (a /resume back) adds nothing.
        s.rebind_conversation(new, "c-2", crate::store::StartSource::Clear, None, None)
            .unwrap();
        s.rebind_conversation(new, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        assert_eq!(s.session_work_links(new).unwrap().len(), 1);
    }

    #[test]
    fn a_rejection_or_other_primary_is_respected_by_the_carry() {
        let s = Store::open_in_memory().unwrap();
        let old = with_conversation(&s, "old", "c-1");
        s.link_session_work(old, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(old).unwrap();
        let rejecting = with_conversation(&s, "rej", "c-x");
        s.reject_session_work(rejecting, WorkTarget::Key("ABC-1"))
            .unwrap();
        s.rebind_conversation(
            rejecting,
            "c-1",
            crate::store::StartSource::Resume,
            None,
            None,
        )
        .unwrap();
        let links = s.session_work_links(rejecting).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].state, "rejected", "sticky");

        let busy = with_conversation(&s, "busy", "c-y");
        s.link_session_work(busy, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        s.rebind_conversation(busy, "c-1", crate::store::StartSource::Resume, None, None)
            .unwrap();
        let links = s.session_work_links(busy).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].ref_key.as_deref(), Some("DEF-2"), "primary stays");
        assert!(!links[1].is_primary);
    }

    /// A tracker item is one target however it is spelled: a rejection made
    /// by item id blocks a carry that names its key, and a repoint merge
    /// does not stack a by-id link beside a by-key one.
    #[test]
    fn a_carry_and_a_repoint_match_the_item_under_either_spelling() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        let item = super::super::test_support::tracker_item(&s, t.id, "10001", "ABC-1", "Pay");

        let rejecting = with_conversation(&s, "rej", "c-x");
        s.reject_session_work(rejecting, WorkTarget::Item(item))
            .unwrap();
        assert!(
            !s.link_resumed_work(rejecting, "ABC-1").unwrap(),
            "a carry by key is blocked by the rejection by id"
        );
        let links = s.session_work_links(rejecting).unwrap();
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].state, "rejected");
        let row = s.get_session_by_id(rejecting).unwrap().unwrap();
        assert_eq!(row.work, None);

        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Item(item), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 1, "one link to the item: {links:?}");
        assert_eq!(links[0].item_id, Some(item));
        assert!(links[0].is_primary);
    }

    #[test]
    fn a_fork_copies_confirmed_links_only() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.reject_session_work(src, WorkTarget::Key("NOPE-1"))
            .unwrap();
        s.link_session_work(src, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        assert_eq!(s.copy_work_links(src, dst).unwrap(), 2);
        let links = s.session_work_links(dst).unwrap();
        assert_eq!(links.len(), 2);
        assert!(links
            .iter()
            .all(|l| l.source == "forked" && l.state == "confirmed"));
        assert_eq!(
            links[0].ref_key.as_deref(),
            Some("DEF-2"),
            "the source's primary"
        );
        assert_eq!(s.copy_work_links(src, dst).unwrap(), 0, "idempotent");
        assert_eq!(
            s.session_work_links(src).unwrap().len(),
            3,
            "source untouched"
        );
    }

    #[test]
    fn reviews_and_workers_inherit_the_parents_primary_work() {
        let s = Store::open_in_memory().unwrap();
        let parent = seed(&s, "parent");
        let review = seed(&s, "review");
        let worker = seed(&s, "worker");
        let own = seed(&s, "own");
        s.link_session_work(parent, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(own, WorkTarget::Key("XYZ-9"), "manual")
            .unwrap();
        s.set_session_kind(review, "review", Some(parent)).unwrap();
        s.inherit_worker_work(worker, parent).unwrap();
        s.inherit_worker_work(own, parent).unwrap();
        for (sid, role) in [(review, "review"), (worker, "worker")] {
            let links = s.session_work_links(sid).unwrap();
            assert_eq!(links.len(), 1, "{role}");
            assert_eq!(
                (
                    links[0].ref_key.as_deref(),
                    links[0].source.as_str(),
                    links[0].role.as_str()
                ),
                (Some("ABC-1"), "inherited", role)
            );
            assert!(links[0].is_primary);
        }
        let own_links = s.session_work_links(own).unwrap();
        assert_eq!(own_links.len(), 1, "a session with its own work keeps it");
        assert_eq!(own_links[0].ref_key.as_deref(), Some("XYZ-9"));
        // A parent without work gives nothing.
        let orphan = seed(&s, "orphan");
        let lone = seed(&s, "lone");
        assert!(!s.inherit_worker_work(orphan, lone).unwrap());
        assert!(s.session_work_links(orphan).unwrap().is_empty());
    }

    #[test]
    fn a_purge_names_and_marks_the_work_that_loses_its_transcripts() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_host("g").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let mk = |name: &str, host: &str| {
            let id = s
                .upsert_session(name, host, None, None, 1, 1, "running", None)
                .unwrap();
            s.conn
                .execute(
                    "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                    rusqlite::params![pid, id],
                )
                .unwrap();
            id
        };
        let ended = mk("ended", "h");
        let live = mk("live", "h");
        let elsewhere = mk("elsewhere", "g");
        s.link_session_work(ended, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(live, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        s.link_session_work(elsewhere, WorkTarget::Key("GHI-3"), "manual")
            .unwrap();
        s.delete_session(ended).unwrap();
        let hosts = vec!["h".to_string()];
        assert_eq!(
            s.work_keys_for_purge(pid, &hosts).unwrap(),
            vec!["ABC-1".to_string(), "DEF-2".to_string()]
        );
        s.delete_session(live).unwrap();
        assert_eq!(s.mark_purged_work_unresumable(pid, &hosts).unwrap(), 2);
        assert!(s
            .ended_work_links_for_key("ABC-1")
            .unwrap()
            .iter()
            .all(|l| !l.resumable));
        assert!(s.session_work_links(elsewhere).unwrap()[0].resumable);
        assert!(s.work_keys_for_purge(pid, &hosts).unwrap().is_empty());
    }

    #[test]
    fn a_repoint_collision_moves_the_targets_live_links_to_the_survivor() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src");
        let dst = seed(&s, "dst");
        s.link_session_work(src, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(dst, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        let links = s.session_work_links(dst).unwrap();
        let keys: Vec<_> = links.iter().map(|l| l.ref_key.clone().unwrap()).collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys.contains(&"ABC-1".to_string()) && keys.contains(&"DEF-2".to_string()));
        assert_eq!(links.iter().filter(|l| l.is_primary).count(), 1);
        assert_eq!(
            links[0].ref_key.as_deref(),
            Some("ABC-1"),
            "the survivor's primary"
        );
    }
}
