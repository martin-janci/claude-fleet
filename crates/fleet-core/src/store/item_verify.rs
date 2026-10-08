//! An item's acceptance conditions and the checks recorded against them
//! (orchestration O3, design 2026-10-07 §6).
//!
//! `work_items.done_when` holds the condition lines (a JSON array, typed by
//! prefix: `ci:<check>`, `review`, `test:<command>`, `person`; see
//! `service::work::verify`). `work_item_verifications` is the journal of
//! checks a PERSON recorded against a line: the newest row per line decides
//! it, and nothing else writes there. Conditions fleet can read for itself
//! (CI, a reviewer's or a tester's run) are derived when asked, never stored.

use super::Store;
use crate::ipc_error::{codes, IpcError};

/// Most condition lines one item holds.
pub const DONE_WHEN_MAX: usize = 20;
/// Longest condition line, in characters.
pub const DONE_WHEN_LINE_MAX_CHARS: usize = 300;
/// Longest note on a recorded check.
pub const VERIFY_NOTE_MAX_CHARS: usize = 500;

/// One check a person recorded against a condition line.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerificationRow {
    pub id: i64,
    pub item_id: i64,
    pub line: String,
    pub ok: bool,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: i64,
}

/// PURE: condition lines as stored: trimmed, non-empty, unique, capped.
/// `E_INVALID` for too many or too long a line, rather than a silent cut.
pub fn normalize_done_when(lines: &[String]) -> Result<Vec<String>, IpcError> {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.is_empty() || out.iter().any(|o| o == t) {
            continue;
        }
        if t.chars().count() > DONE_WHEN_LINE_MAX_CHARS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("a done_when line holds at most {DONE_WHEN_LINE_MAX_CHARS} characters"),
            ));
        }
        out.push(t.to_string());
    }
    if out.len() > DONE_WHEN_MAX {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("an item holds at most {DONE_WHEN_MAX} done_when lines"),
        ));
    }
    Ok(out)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Replace an item's condition lines. Answers whether anything changed;
    /// a change bumps the item (so the views refetch) and lands in its
    /// mission's log as `done_when`.
    pub fn set_item_done_when(
        &self,
        item_id: i64,
        lines: &[String],
        actor: &str,
    ) -> Result<bool, IpcError> {
        let lines = normalize_done_when(lines)?;
        let item = self
            .get_work_item(item_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("work item {item_id}")))?;
        if item.done_when == lines {
            return Ok(false);
        }
        let json = (!lines.is_empty())
            .then(|| serde_json::to_string(&lines).unwrap_or_else(|_| "[]".into()));
        self.conn.execute(
            "UPDATE work_items SET done_when = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![json, now_unix(), item_id],
        )?;
        self.emit_work_item(item_id, super::tracker_items::SessionChange::default())?;
        self.log_item_event(
            item_id,
            "done_when",
            actor,
            Some(serde_json::json!({ "lines": lines })),
        )?;
        Ok(true)
    }

    /// Record a person's check of one condition line.
    pub fn record_verification(
        &self,
        item_id: i64,
        line: &str,
        ok: bool,
        actor: &str,
        note: Option<&str>,
    ) -> Result<VerificationRow, IpcError> {
        let note: Option<String> = note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(VERIFY_NOTE_MAX_CHARS).collect());
        let at = now_unix();
        self.conn.execute(
            "INSERT INTO work_item_verifications (item_id, line, ok, actor, note, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![item_id, line, ok as i64, actor, note, at],
        )?;
        let id = self.conn.last_insert_rowid();
        self.emit_work_item(item_id, super::tracker_items::SessionChange::default())?;
        self.log_item_event(
            item_id,
            "verify",
            actor,
            Some(serde_json::json!({ "line": line, "ok": ok })),
        )?;
        Ok(VerificationRow {
            id,
            item_id,
            line: line.to_string(),
            ok,
            actor: actor.to_string(),
            note,
            at,
        })
    }

    /// The newest recorded check of each line of `item_id`.
    pub fn latest_verifications(&self, item_id: i64) -> Result<Vec<VerificationRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, item_id, line, ok, actor, note, at FROM work_item_verifications v \
             WHERE item_id = ?1 AND id = (SELECT MAX(id) FROM work_item_verifications \
                                          WHERE item_id = v.item_id AND line = v.line) \
             ORDER BY id",
        )?;
        let rows = stmt.query_map(rusqlite::params![item_id], |r| {
            Ok(VerificationRow {
                id: r.get(0)?,
                item_id: r.get(1)?,
                line: r.get(2)?,
                ok: r.get::<_, i64>(3)? != 0,
                actor: r.get(4)?,
                note: r.get(5)?,
                at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests;
