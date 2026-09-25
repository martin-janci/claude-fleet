//! Local work items (work graph M11.1, "Name this work…"): work that has a
//! title and no tracker ticket. The table and its unique local key are
//! migration 046's; this adds the writers a person reaches — name a
//! session's work, rename a local item — and the reader that lists them.
//!
//! A local item has no org of its own (`Store::item_org` is `None`): a link
//! to one takes its session's org, so who may see an item is decided by its
//! links (`service::work::local`), never here.

use super::work::{link_columns_prefixed, map_item, map_link, ITEM_COLUMNS, LINK_COLUMN_COUNT};
use super::{now_unix, Store, WorkItemRow, WorkLinkRow};
use crate::ipc_error::{codes, IpcError};

/// Longest title "Name this work…" accepts. Shorter than the store's
/// general [`super::work::WORK_TITLE_MAX_CHARS`] (hooks and tests write up
/// to that): a name is a sidebar group header, not a description.
pub const LOCAL_WORK_TITLE_MAX_CHARS: usize = 120;

/// Validate a title a person gave local work: trimmed, 1 to
/// [`LOCAL_WORK_TITLE_MAX_CHARS`] characters, no control character.
pub fn validate_local_work_title(raw: &str) -> Result<String, IpcError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work title must not be empty",
        ));
    }
    if t.chars().count() > LOCAL_WORK_TITLE_MAX_CHARS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("work title longer than {LOCAL_WORK_TITLE_MAX_CHARS} characters"),
        ));
    }
    if t.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "work title must not contain control characters",
        ));
    }
    Ok(t.to_string())
}

/// One confirmed link to a local item, with the live session it is on
/// (`None` once the link ended).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalItemLink {
    pub item_id: i64,
    pub link: WorkLinkRow,
    pub session_id: Option<i64>,
    pub session_host: Option<String>,
}

impl Store {
    /// Every local work item, most recently changed first.
    pub fn local_work_items(&self) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items WHERE source = 'local' \
             ORDER BY updated_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The confirmed links (live and ended) to local items — every local
    /// item, or only `item_id` — each with its live session and that
    /// session's host.
    pub fn local_item_links(&self, item_id: Option<i64>) -> Result<Vec<LocalItemLink>, IpcError> {
        let cols = link_columns_prefixed("l");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols}, s.id, s.host_alias FROM work_links l \
             JOIN work_items i ON i.id = l.item_id AND i.source = 'local' \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                                         AND l.ended_at IS NULL \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.state = 'confirmed' AND (?1 IS NULL OR l.item_id = ?1) \
             ORDER BY l.id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![item_id], |r| {
            let link = map_link(r)?;
            Ok(LocalItemLink {
                item_id: link.item_id.unwrap_or_default(),
                link,
                session_id: r.get(LINK_COLUMN_COUNT)?,
                session_host: r.get(LINK_COLUMN_COUNT + 1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every tracker item that answers to `key` (normalised) by key or
    /// alias, in any tracker — unlike `tracker_item_for_key`, which answers
    /// only when exactly one tracker has it.
    pub fn tracker_items_with_key(&self, key: &str) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items WHERE tracker_id IS NOT NULL AND \
               (key = ?1 OR EXISTS (SELECT 1 FROM json_each(COALESCE(aliases, '[]')) \
                                    WHERE value = ?1)) \
             ORDER BY id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![key], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// "Name this work…": create a NEW local item (`title` validated by
    /// [`validate_local_work_title`], `key` already normalised and checked
    /// free by the caller) and link `session_id` to it — manual, confirmed,
    /// explicit, and primary only when the session has no primary work yet.
    /// One transaction; emits `work:item` and the session's row.
    ///
    /// `E_EXISTS` when a local item already carries `key` (the unique
    /// index); `E_NOTFOUND` for a session row that does not exist.
    pub fn name_session_work(
        &self,
        session_id: i64,
        key: Option<&str>,
        title: &str,
    ) -> Result<(WorkItemRow, WorkLinkRow), IpcError> {
        let title = validate_local_work_title(title)?;
        if let Some(k) = key {
            if self.local_work_item_by_key(k)?.is_some() {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!("work key {k} already names local work"),
                ));
            }
        }
        let participant = self.work_participant(session_id)?;
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, created_at, updated_at) \
             VALUES ('local', ?1, ?2, ?3, ?3)",
            rusqlite::params![key, title, now],
        )?;
        let item_id = self.conn.last_insert_rowid();
        let has_primary: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL AND is_primary = 1)",
            rusqlite::params![participant],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                     is_primary, created_at, decided_at, strength, \
                                     claude_session_id) \
             VALUES (?1, NULL, ?2, 'confirmed', 'manual', ?3, ?4, ?4, 'explicit', \
                     (SELECT claude_session_id FROM sessions WHERE id = ?5))",
            rusqlite::params![item_id, participant, (!has_primary) as i64, now, session_id],
        )?;
        let link_id = self.conn.last_insert_rowid();
        self.bump_session_for_work(session_id)?;
        tx.commit()?;
        // The row first: a phone that reads `work:item` finds the session
        // already showing it.
        self.emit_session(session_id)?;
        // `work:item` alone: the one row that shows the item was just sent.
        self.emit_work_item(item_id, super::tracker_items::SessionChange::default())?;
        let item = self
            .get_work_item(item_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))?;
        let link = self
            .get_work_link(link_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished after write"))?;
        Ok((item, link))
    }

    /// Rename a LOCAL item. `None` when `item_id` is not a local item (a
    /// tracker's ticket is the tracker's to name). Emits `work:item`, and
    /// `session:updated` for every live session whose row shows it.
    pub fn rename_local_work_item(
        &self,
        item_id: i64,
        title: &str,
    ) -> Result<Option<WorkItemRow>, IpcError> {
        let title = validate_local_work_title(title)?;
        let Some(before) = self.get_work_item(item_id)? else {
            return Ok(None);
        };
        if before.source != "local" {
            return Ok(None);
        }
        if before.title == title {
            return Ok(Some(before));
        }
        self.conn.execute(
            "UPDATE work_items SET title = ?1, updated_at = ?2 WHERE id = ?3 AND source = 'local'",
            rusqlite::params![title, now_unix(), item_id],
        )?;
        // A title shows as a row's primary work and as its top suggestion;
        // a rejection lists keys only.
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: true,
                suggested: true,
                rejected: false,
            },
        )?;
        self.get_work_item(item_id)
    }
}

#[cfg(test)]
mod tests;
