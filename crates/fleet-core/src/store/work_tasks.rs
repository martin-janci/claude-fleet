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

/// Most people one local item names as its assignees.
pub const ASSIGNEES_MAX: usize = 10;
/// Longest assignee name, in characters.
pub const ASSIGNEE_MAX_CHARS: usize = 80;

/// A person's edit of a local item: each field `None` is left as it is.
/// `notes: Some("")` and `assignees: Some(&[])` clear them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ItemEdit<'a> {
    pub title: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub assignees: Option<&'a [String]>,
}

/// Assignees a person typed: trimmed, empty ones dropped, each name once
/// (first spelling wins, compared case-insensitively), at most
/// [`ASSIGNEES_MAX`] of at most [`ASSIGNEE_MAX_CHARS`] characters, no
/// control character.
pub fn validate_assignees(raw: &[String]) -> Result<Vec<String>, IpcError> {
    let mut out: Vec<String> = Vec::new();
    for a in raw.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
        if a.chars().count() > ASSIGNEE_MAX_CHARS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("an assignee is longer than {ASSIGNEE_MAX_CHARS} characters"),
            ));
        }
        if a.chars().any(char::is_control) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "an assignee must not contain control characters",
            ));
        }
        if !out.iter().any(|o| o.to_lowercase() == a.to_lowercase()) {
            out.push(a.to_string());
        }
    }
    if out.len() > ASSIGNEES_MAX {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("at most {ASSIGNEES_MAX} assignees"),
        ));
    }
    Ok(out)
}

/// An agent's proposed subtask.
#[derive(Debug, Clone, Copy)]
pub struct Proposal<'a> {
    pub parent_id: i64,
    pub title: &'a str,
    pub notes: Option<&'a str>,
    pub why: Option<&'a str>,
    pub proposed_by: &'a str,
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
    ///
    /// **The mirror's `title` and `notes` are the dispatch PROMPT** — the
    /// first line of it and the whole of it, respectively — which is what
    /// makes them §4.3 content of both ends of the dispatch rather than
    /// shared work structure (multi-user M1, T5's review). Any reader of a
    /// mirror's text has to be fenced on the job itself; `Graph::job_states`
    /// is where that fence is applied, and the `open_proposals` note beside
    /// it records the reasoning that used to exempt "every item's title" and
    /// should never have covered this one.
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

    /// Every job-mirror item with the **whole `tasks` row** behind it, keyed
    /// by the mirror item's id.
    ///
    /// It used to answer `(item id, state)` and nothing else, and that is why
    /// it is written this way now (multi-user M1, the review of main's new
    /// `Graph` fields). A `tasks` row is a DISPATCH: both its ends are
    /// sessions, and whether a caller may read it is
    /// `service::tasks::task_visible_in_scope_pure` — which needs the row,
    /// not a state string. Answering the state alone made the fence
    /// unaskable, so `Graph::build` served another person's job state (and,
    /// through `JobView`, its `result`) to anyone who could see the work item.
    ///
    /// One query, as before: the fence is applied in memory by the caller.
    pub fn job_tasks_by_item(&self) -> Result<HashMap<i64, TaskRow>, IpcError> {
        // The task columns FIRST, because `map_task_row` reads them by index;
        // the item id rides last, at `TASK_COLUMNS`'s length.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {}, w.id FROM work_items w JOIN tasks t ON t.id = w.task_id",
            super::rows::task_columns_t()
        ))?;
        let item_id = super::rows::TASK_COLUMNS.split(',').count();
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(item_id)?, super::rows::map_task_row(r)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// A subtask an agent proposes; a person accepts or rejects it.
    pub fn propose_subtask(&self, p: &Proposal<'_>) -> Result<WorkItemRow, IpcError> {
        let title = validate_local_work_title(p.title)?;
        self.parent_for_new_child(p.parent_id)?;
        let norm = |t: &str| {
            t.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let (open, rejected_same): (usize, bool) = {
            let siblings = self.native_children(p.parent_id)?;
            let open = siblings
                .iter()
                .filter(|c| c.proposal_state.as_deref() == Some("proposed"))
                .count();
            let same = siblings.iter().any(|c| {
                c.proposal_state.as_deref() == Some("rejected") && norm(&c.title) == norm(&title)
            });
            (open, same)
        };
        if rejected_same {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("\"{title}\" was already proposed here and rejected"),
            ));
        }
        // A proposal under a mission's member joins the mission: refuse it
        // before it is written when the mission has no room.
        let mission = self.mission_for_new_proposal(p.parent_id)?;
        if open >= PROPOSALS_OPEN_CAP {
            return Err(IpcError::new(
                codes::E_LIMIT,
                format!("{PROPOSALS_OPEN_CAP} proposals already wait for a decision on this task"),
            ));
        }
        let notes = p
            .notes
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(cut_brief);
        let why = p
            .why
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(cut_brief);
        let by: String = p
            .proposed_by
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect();
        let item = self.insert_native(&NativeRow {
            origin: "proposed",
            title: &title,
            parent_id: Some(p.parent_id),
            project_id: None,
            notes: notes.as_deref(),
            task_id: None,
            status: "todo",
            status_set_by: None,
            proposal_state: Some("proposed"),
            proposed_by: Some(&by),
            proposal_why: why.as_deref(),
        })?;
        match mission {
            Some(m) => {
                self.join_mission(m, item.id, &by)?;
                self.get_work_item(item.id)?
                    .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "proposal vanished"))
            }
            None => Ok(item),
        }
    }

    /// A person decides a proposal, once.
    pub fn decide_proposal(&self, item_id: i64, accept: bool) -> Result<WorkItemRow, IpcError> {
        let now = now_unix();
        let wrote = self.conn.execute(
            "UPDATE work_items SET proposal_state = ?1, updated_at = ?2 \
              WHERE id = ?3 AND origin = 'proposed' AND proposal_state = 'proposed'",
            rusqlite::params![if accept { "accepted" } else { "rejected" }, now, item_id],
        )? == 1;
        if !wrote {
            return match self.get_work_item(item_id)? {
                None => Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("work item {item_id} not found"),
                )),
                Some(_) => Err(IpcError::new(
                    codes::E_INVALID,
                    "that item is not a proposal waiting for a decision",
                )),
            };
        }
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: false,
                suggested: false,
                rejected: !accept,
            },
        )?;
        self.get_work_item(item_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after update"))
    }

    /// Redesign 6.9: a person's Merge on a proposal that duplicates
    /// `into` (K4's "may duplicate"): what hangs on the proposal moves to
    /// the task it duplicates, and the proposal is closed as rejected (its
    /// archive). Its subtasks are re-parented under `into`, and its session
    /// links (live and ended) point at `into` — a session already live on
    /// `into` keeps that link and the proposal's one ends. One transaction.
    /// Returns `into` as it is after the merge. `E_INVALID` for an item
    /// that is not a proposal waiting for a decision, a merge into itself
    /// or into one of its own subtasks, or subtasks that would land under a
    /// subtask; `E_NOTFOUND` for an unknown id.
    pub fn merge_proposal_into(
        &self,
        item_id: i64,
        into: i64,
    ) -> Result<(WorkItemRow, MergedProposal), IpcError> {
        let now = now_unix();
        let notfound =
            |id: i64| IpcError::new(codes::E_NOTFOUND, format!("work item {id} not found"));
        let proposal = self
            .get_work_item(item_id)?
            .ok_or_else(|| notfound(item_id))?;
        if proposal.origin.as_deref() != Some("proposed")
            || proposal.proposal_state.as_deref() != Some("proposed")
        {
            return Err(IpcError::new(
                codes::E_INVALID,
                "that item is not a proposal waiting for a decision",
            ));
        }
        let target = self.get_work_item(into)?.ok_or_else(|| notfound(into))?;
        // Not into itself, nor into anything under it.
        let mut up = Some(target.id);
        while let Some(id) = up {
            if id == item_id {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "a proposal cannot be merged into itself or one of its own subtasks",
                ));
            }
            up = self.get_work_item(id)?.and_then(|r| r.parent_id);
        }
        let children = self.native_children(item_id)?;
        if !children.is_empty() && target.parent_id.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "its subtasks cannot move under a subtask; keep both instead",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let moved_subtasks = self.conn.execute(
            "UPDATE work_items SET parent_id = ?1, updated_at = ?2 WHERE parent_id = ?3",
            rusqlite::params![into, now, item_id],
        )?;
        // A participant already live on `into` keeps that link; the
        // proposal's own live one ends rather than doubling it.
        let ended_links = self.conn.execute(
            "UPDATE work_links SET ended_at = ?1 \
              WHERE item_id = ?2 AND ended_at IS NULL AND participant_id IN \
                (SELECT participant_id FROM work_links \
                  WHERE item_id = ?3 AND ended_at IS NULL AND participant_id IS NOT NULL)",
            rusqlite::params![now, item_id, into],
        )?;
        let moved_links = self.conn.execute(
            "UPDATE work_links SET item_id = ?1 WHERE item_id = ?2",
            rusqlite::params![into, item_id],
        )?;
        self.conn.execute(
            "UPDATE work_items SET proposal_state = 'rejected', updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, item_id],
        )?;
        tx.commit()?;
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: false,
                suggested: false,
                rejected: true,
            },
        )?;
        self.emit_work_item(
            into,
            super::tracker_items::SessionChange {
                primary: moved_links > 0,
                suggested: false,
                rejected: false,
            },
        )?;
        let row = self
            .get_work_item(into)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after merge"))?;
        Ok((
            row,
            MergedProposal {
                subtasks: moved_subtasks,
                links: moved_links,
                ended_links,
            },
        ))
    }

    /// A person edits a LOCAL item's title, notes and assignees (task
    /// editing). `None` when `item_id` is not a local item: a tracker's
    /// ticket is its tracker's to edit. A job mirror's notes are its
    /// dispatch prompt ([`Self::create_agent_task_item`]), so editing them
    /// is `E_INVALID`. Nothing changed answers the row as it is, unwritten.
    pub fn edit_local_item(
        &self,
        item_id: i64,
        edit: &ItemEdit<'_>,
    ) -> Result<Option<WorkItemRow>, IpcError> {
        let title = edit.title.map(validate_local_work_title).transpose()?;
        let assignees = edit.assignees.map(validate_assignees).transpose()?;
        let Some(before) = self.get_work_item(item_id)? else {
            return Ok(None);
        };
        if before.source != "local" {
            return Ok(None);
        }
        let notes = edit.notes.map(|n| {
            let n = n.trim();
            (!n.is_empty()).then(|| cut_brief(n))
        });
        if notes.is_some() && before.origin.as_deref() == Some("agent") {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a delegated job's notes are its prompt and cannot be edited",
            ));
        }
        let title = title.filter(|t| *t != before.title);
        let notes = notes.filter(|n| *n != before.notes);
        let assignees = assignees.filter(|a| *a != before.assignees);
        if title.is_none() && notes.is_none() && assignees.is_none() {
            return Ok(Some(before));
        }
        self.conn.execute(
            "UPDATE work_items SET title = COALESCE(?1, title), \
                    notes = CASE WHEN ?2 THEN ?3 ELSE notes END, \
                    assignees = CASE WHEN ?4 THEN ?5 ELSE assignees END, \
                    updated_at = ?6 \
              WHERE id = ?7 AND source = 'local'",
            rusqlite::params![
                title,
                notes.is_some(),
                notes.clone().flatten(),
                assignees.is_some(),
                assignees
                    .as_ref()
                    .filter(|a| !a.is_empty())
                    .and_then(|a| serde_json::to_string(a).ok()),
                now_unix(),
                item_id
            ],
        )?;
        // A title shows as a row's primary work and as its top suggestion,
        // as for a rename; notes and assignees show on the item alone.
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: title.is_some(),
                suggested: title.is_some(),
                rejected: false,
            },
        )?;
        self.get_work_item(item_id)
    }

    /// Every native child of `parent_id`, proposals included, oldest first.
    ///
    /// `origin = 'agent'` rows are JOB MIRRORS, and their title and notes are
    /// the dispatch prompt ([`Self::create_agent_task_item`]). A caller that
    /// serves a child's text must fence those on the job
    /// (`Graph::job_states`); this query applies no fence of its own.
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

mod tree;
pub use tree::{TreeEntry, TreeRef, ACCEPT_UNDO_SECS};

/// What [`Store::merge_proposal_into`] moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergedProposal {
    /// Subtasks re-parented under the task merged into.
    pub subtasks: usize,
    /// Session links (live and ended) now pointing at it.
    pub links: usize,
    /// The proposal's live links that ended, their session already live on it.
    pub ended_links: usize,
}

#[cfg(test)]
mod tests;
