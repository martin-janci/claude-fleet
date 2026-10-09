//! What a finished attempt left behind (orchestration O3, design 2026-10-07
//! §6): the worker's structured report and the evidence fleet read from git
//! itself, never from the report.
//!
//! - `tasks.result_json` holds a [`TaskReport`]: what the worker SAYS it did,
//!   parsed from the fenced JSON block after its done marker
//!   (`service::work::report::parse_report`). Absent when the worker printed
//!   only a paragraph, which stays in `tasks.result` as before.
//! - `tasks.evidence_json` holds a [`TaskEvidence`]: the commits and changed
//!   files git reports in the worker's checkout against its base, read once
//!   when the attempt finished (`service::work::report::collect_evidence`).
//!
//! Both are capped on the way in, so a row never grows past a few kilobytes.

use super::Store;
use crate::events::EventBus;
use crate::ipc_error::IpcError;

/// The outcomes a worker may report; anything else reads as `partial`.
pub const REPORT_OUTCOMES: &[&str] = &["done", "partial", "blocked", "failed"];
/// Most entries one report list keeps.
pub const REPORT_LIST_MAX: usize = 20;
/// Longest entry of a report list, in characters.
pub const REPORT_ENTRY_MAX_CHARS: usize = 500;
/// Most commits and files one evidence reading keeps (the totals count all).
pub const EVIDENCE_COMMITS_MAX: usize = 50;
pub const EVIDENCE_FILES_MAX: usize = 200;

/// A worker's own account of an attempt. Every field is what the worker
/// said; nothing in it is checked, and nothing reads it as proof.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct TaskReport {
    #[serde(default)]
    pub summary: String,
    /// One of [`REPORT_OUTCOMES`].
    #[serde(default)]
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests_run: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<String>,
    /// `low` | `medium` | `high`, or the number the worker gave, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

/// One commit on the attempt's branch that its base does not have.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvidenceCommit {
    pub sha: String,
    pub subject: String,
}

/// One file the branch changed against its base. Binary files have no
/// line counts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvidenceFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
}

/// What git said about the attempt's checkout when it finished. An absent
/// field is "not observed", never "nothing": `error` says why git could not
/// answer at all.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct TaskEvidence {
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The ref the branch was compared against (`origin/HEAD`'s target, or
    /// `origin/main` / `origin/master`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_base: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<EvidenceCommit>,
    #[serde(default)]
    pub commits_total: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<EvidenceFile>,
    #[serde(default)]
    pub files_total: u32,
    /// Tracked files that differ from `HEAD`: work the commits do not hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Decode a stored JSON column; text fleet did not write reads as absent.
pub(super) fn decode<T: serde::de::DeserializeOwned>(raw: Option<String>) -> Option<T> {
    raw.and_then(|t| serde_json::from_str(&t).ok())
}

impl Store {
    /// Store the worker's report on a task (any state: the report arrives
    /// with the flip to `done`, which already happened).
    pub fn set_task_report(&self, task_id: i64, report: &TaskReport) -> Result<(), IpcError> {
        let json = serde_json::to_string(report)
            .map_err(|e| IpcError::new(crate::ipc_error::codes::E_INTERNAL, e.to_string()))?;
        self.conn.execute(
            "UPDATE tasks SET result_json = ?1 WHERE id = ?2",
            rusqlite::params![json, task_id],
        )?;
        self.emit_task_row(task_id)
    }

    /// Store what git said about a finished task's checkout.
    pub fn set_task_evidence(&self, task_id: i64, ev: &TaskEvidence) -> Result<(), IpcError> {
        let json = serde_json::to_string(ev)
            .map_err(|e| IpcError::new(crate::ipc_error::codes::E_INTERNAL, e.to_string()))?;
        self.conn.execute(
            "UPDATE tasks SET evidence_json = ?1 WHERE id = ?2",
            rusqlite::params![json, task_id],
        )?;
        self.emit_task_row(task_id)
    }

    /// The newest attempt at `item_id` in `role`, open or not.
    pub fn latest_item_task(
        &self,
        item_id: i64,
        role: &str,
    ) -> Result<Option<super::TaskRow>, IpcError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM tasks WHERE work_item_id = ?1 \
                       AND (role = ?2 OR (?2 = 'implement' AND role IS NULL)) \
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    super::rows::TASK_COLUMNS
                ),
                rusqlite::params![item_id, role],
                super::rows::map_task_row,
            )
            .optional()?)
    }

    /// The freshest PR reading among the sessions that worked on `item_id`:
    /// its runs' workers and every session linked to it. `None` when none
    /// of them has a PR reading.
    pub fn item_pr_evidence(
        &self,
        item_id: i64,
    ) -> Result<Option<(crate::service::outcome::PrEvidence, Option<i64>)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT pr_evidence, pr_checked_at FROM sessions \
             WHERE pr_evidence IS NOT NULL AND id IN ( \
               SELECT worker_session_id FROM tasks WHERE work_item_id = ?1 \
               UNION SELECT p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id \
                 WHERE l.item_id = ?1 AND l.state = 'confirmed') \
             ORDER BY pr_checked_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![item_id], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?))
        })?;
        for row in rows {
            let (raw, at) = row?;
            if let Some(ev) = super::rows::decode_pr_evidence(raw) {
                return Ok(Some((ev, at)));
            }
        }
        Ok(None)
    }

    fn emit_task_row(&self, task_id: i64) -> Result<(), IpcError> {
        if let Some(r) = self.get_task(task_id)? {
            self.bus.task_updated(&r);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
