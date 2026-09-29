//! Native work items (design 2026-09-29, shared work context): tasks and
//! subtasks written in fleet (`origin = 'manual'`), agent proposals
//! (`'proposed'`) and the mirror of a dispatched job (`'agent'`). Every one is
//! a local item with a `TASK-<id>` key, so the key-driven start path and
//! branch detection work on them unchanged. Depth is one: a native subtask
//! is never a parent.
//!
//! The only writers of `origin`, `project_id`, `notes`, `task_id` and the
//! proposal columns, and (with `work_status.rs`) of `status_set_by`:
//! `'task'` is written here only.

use super::work::{map_item, ITEM_COLUMNS};
use super::work_local::{validate_local_work_title, LOCAL_WORK_TITLE_MAX_CHARS};
use super::{now_unix, Store, TaskRow, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

pub const TASK_KEY_PREFIX: &str = "TASK";
/// Open proposals one parent may hold (counter-review risk 2).
pub const PROPOSALS_OPEN_CAP: usize = 10;

/// A dispatched job's state as a work status.
pub fn job_status(state: &str) -> &'static str {
    match state {
        "queued" => "todo",
        "running" => "in_progress",
        _ => "done",
    }
}

/// What a person (or an agent's proposal) writes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeItem<'a> {
    pub title: &'a str,
    pub parent_id: Option<i64>,
    pub project_id: Option<i64>,
    pub notes: Option<&'a str>,
}

pub(super) fn cut_brief(s: &str) -> String {
    s.chars()
        .take(crate::service::work::handover::BRIEF_MAX_CHARS)
        .collect()
}

fn job_title(prompt: &str) -> String {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Delegated job");
    first
        .chars()
        .filter(|c| !c.is_control())
        .take(LOCAL_WORK_TITLE_MAX_CHARS)
        .collect()
}

/// Everything `insert_native` writes.
pub(super) struct NativeRow<'a> {
    pub origin: &'a str,
    pub title: &'a str,
    pub parent_id: Option<i64>,
    pub project_id: Option<i64>,
    pub notes: Option<&'a str>,
    pub task_id: Option<i64>,
    pub status: &'a str,
    pub status_set_by: Option<&'a str>,
    pub proposal_state: Option<&'a str>,
    pub proposed_by: Option<&'a str>,
    pub proposal_why: Option<&'a str>,
}

impl Store {
    /// The parent a new native child may hang under.
    pub(super) fn parent_for_new_child(&self, parent_id: i64) -> Result<WorkItemRow, IpcError> {
        let p = self.get_work_item(parent_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {parent_id} not found"),
            )
        })?;
        let native = matches!(p.origin.as_deref(), Some("manual" | "proposed" | "agent"));
        if native && p.parent_id.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{} is itself a subtask; add the subtask to its parent instead",
                    p.key.as_deref().unwrap_or("that item")
                ),
            ));
        }
        Ok(p)
    }

    fn check_project(&self, project_id: Option<i64>) -> Result<(), IpcError> {
        if let Some(pid) = project_id {
            let known: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                [pid],
                |r| r.get(0),
            )?;
            if !known {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("project {pid} not found"),
                ));
            }
        }
        Ok(())
    }

    /// Insert one native row and give it its `TASK-<id>` key, in one
    /// transaction; emits the row.
    pub(super) fn insert_native(&self, r: &NativeRow<'_>) -> Result<WorkItemRow, IpcError> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, origin, parent_id, project_id, notes, task_id, \
                                     status_category, status_set_by, status_set_at, proposal_state, \
                                     proposed_by, proposal_why, created_at, updated_at) \
             VALUES ('local', NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                     CASE WHEN ?8 IS NULL THEN NULL ELSE ?12 END, ?9, ?10, ?11, ?12, ?12)",
            rusqlite::params![
                r.title,
                r.origin,
                r.parent_id,
                r.project_id,
                r.notes,
                r.task_id,
                r.status,
                r.status_set_by,
                r.proposal_state,
                r.proposed_by,
                r.proposal_why,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE work_items SET key = ?1 WHERE id = ?2",
            rusqlite::params![format!("{TASK_KEY_PREFIX}-{id}"), id],
        )?;
        tx.commit()?;
        self.emit_work_item(
            id,
            super::tracker_items::SessionChange {
                primary: false,
                suggested: false,
                rejected: false,
            },
        )?;
        self.get_work_item(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))
    }

    /// A task or subtask a person writes: `origin = 'manual'`, `todo`.
    pub fn create_native_item(&self, n: &NativeItem<'_>) -> Result<WorkItemRow, IpcError> {
        let title = validate_local_work_title(n.title)?;
        if let Some(p) = n.parent_id {
            self.parent_for_new_child(p)?;
        }
        self.check_project(n.project_id)?;
        let notes = n
            .notes
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(cut_brief);
        self.insert_native(&NativeRow {
            origin: "manual",
            title: &title,
            parent_id: n.parent_id,
            project_id: n.project_id,
            notes: notes.as_deref(),
            task_id: None,
            status: "todo",
            status_set_by: None,
            proposal_state: None,
            proposed_by: None,
            proposal_why: None,
        })
    }

    /// Mirror a dispatched job as an `agent` subtask, once per job.
    pub fn create_agent_task_item(
        &self,
        task: &TaskRow,
        parent_item_id: Option<i64>,
        project_id: Option<i64>,
    ) -> Result<WorkItemRow, IpcError> {
        if let Some(existing) = self.work_item_for_task(task.id)? {
            return Ok(existing);
        }
        let prompt = task.prompt.clone().unwrap_or_default();
        self.insert_native(&NativeRow {
            origin: "agent",
            title: &job_title(&prompt),
            parent_id: parent_item_id,
            project_id,
            notes: Some(&cut_brief(&prompt)),
            task_id: Some(task.id),
            status: job_status(&task.state),
            status_set_by: Some("task"),
            proposal_state: None,
            proposed_by: None,
            proposal_why: None,
        })
    }

    pub fn work_item_for_task(&self, task_id: i64) -> Result<Option<WorkItemRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM work_items WHERE task_id = ?1"),
                [task_id],
                map_item,
            )
            .optional()?)
    }

    /// A job moved: its item follows unless a person set a status.
    pub fn set_item_status_from_task(&self, item_id: i64, status: &str) -> Result<bool, IpcError> {
        let now = now_unix();
        let wrote = self.conn.execute(
            "UPDATE work_items SET status_category = ?1, status_set_by = 'task', \
                    status_set_at = ?2, status_changed_at = ?2, updated_at = ?2 \
              WHERE id = ?3 AND source = 'local' \
                AND COALESCE(status_set_by, '') <> 'person' \
                AND NOT (status_category = ?1 AND status_set_by = 'task')",
            rusqlite::params![status, now, item_id],
        )? == 1;
        if wrote {
            self.emit_work_item(
                item_id,
                super::tracker_items::SessionChange {
                    primary: true,
                    suggested: false,
                    rejected: false,
                },
            )?;
        }
        Ok(wrote)
    }

    pub fn job_states_by_item(&self) -> Result<HashMap<i64, String>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT w.id, t.state FROM work_items w JOIN tasks t ON t.id = w.task_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// Every native child of `parent_id`, proposals included, oldest first.
    pub fn native_children(&self, parent_id: i64) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
              WHERE parent_id = ?1 AND origin IN ('manual', 'proposed', 'agent') \
              ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([parent_id], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests;
