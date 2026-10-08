//! A proposed TREE of subtasks, accepting many proposals at once, and
//! undoing that acceptance (orchestration O2, design 2026-10-07 §10).
//!
//! A tree is several proposals under one parent with edges between them: an
//! agent (and later the planner) proposes the plan in one call, a person
//! accepts it in one. Every proposal joins the parent's mission, when the
//! parent has one, so the plan lands in the mission it was made for.
//!
//! Atomic by validation: every refusal a tree can meet is checked before the
//! first row is written, and a write that still fails takes the rows already
//! written back out.

use super::super::tracker_items::SessionChange;
use super::super::{now_unix, Store, WorkItemRow};
use super::{Proposal, PROPOSALS_OPEN_CAP};
use crate::ipc_error::{codes, IpcError};
use serde::{Deserialize, Serialize};

/// How long after an accept a person may still undo it, seconds.
pub const ACCEPT_UNDO_SECS: i64 = 600;

/// What a proposed subtask waits for: an earlier entry of the same tree (by
/// its index), or an existing item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TreeRef {
    /// The `n`th entry of this tree, counted from 0; only an EARLIER one,
    /// so a tree can never hold a cycle.
    Entry(usize),
    /// An existing work item, of the parent's org.
    Item(i64),
}

/// One entry of a proposed tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeEntry {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<TreeRef>,
}

impl Store {
    /// Propose `entries` under `parent_id`, with their edges (`source =
    /// proposal`). Answers the new items in entry order.
    pub fn propose_tree(
        &self,
        parent_id: i64,
        entries: &[TreeEntry],
        proposed_by: &str,
    ) -> Result<Vec<WorkItemRow>, IpcError> {
        if entries.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a tree needs at least one entry",
            ));
        }
        self.parent_for_new_child(parent_id)?;
        let open = self
            .native_children(parent_id)?
            .iter()
            .filter(|c| c.proposal_state.as_deref() == Some("proposed"))
            .count();
        if open + entries.len() > PROPOSALS_OPEN_CAP {
            return Err(IpcError::new(
                codes::E_LIMIT,
                format!(
                    "{open} proposals already wait on this task; a tree may bring it to \
                     {PROPOSALS_OPEN_CAP}"
                ),
            ));
        }
        for (i, e) in entries.iter().enumerate() {
            super::super::work_local::validate_local_work_title(&e.title)?;
            for d in &e.depends_on {
                match *d {
                    TreeRef::Entry(j) if j >= i => {
                        return Err(IpcError::new(
                            codes::E_INVALID,
                            format!("entry {i} may wait only for an earlier entry, not {j}"),
                        ))
                    }
                    TreeRef::Entry(_) => {}
                    TreeRef::Item(id) => {
                        if self.get_work_item(id)?.is_none() {
                            return Err(IpcError::new(
                                codes::E_NOTFOUND,
                                format!("work item {id} not found"),
                            ));
                        }
                        if self.item_org(id)? != self.item_org(parent_id)? {
                            return Err(IpcError::new(
                                codes::E_FORBIDDEN,
                                format!("work item {id} belongs to another organisation"),
                            ));
                        }
                    }
                }
            }
        }
        let mission = self.item_mission(parent_id)?;
        if let Some(m) = mission {
            self.check_mission_room(m, entries.len())?;
        }
        let mut made: Vec<WorkItemRow> = Vec::with_capacity(entries.len());
        let written = (|| -> Result<(), IpcError> {
            for e in entries {
                let item = self.propose_subtask(&Proposal {
                    parent_id,
                    title: &e.title,
                    notes: e.notes.as_deref(),
                    why: e.why.as_deref(),
                    proposed_by,
                })?;
                made.push(item);
            }
            for (i, e) in entries.iter().enumerate() {
                for d in &e.depends_on {
                    let on = match *d {
                        TreeRef::Entry(j) => made[j].id,
                        TreeRef::Item(id) => id,
                    };
                    self.add_item_dep(made[i].id, on, "proposal", proposed_by)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = written {
            for item in &made {
                let _ = self
                    .conn
                    .execute("DELETE FROM work_items WHERE id = ?1", [item.id]);
            }
            return Err(e);
        }
        // `propose_subtask` already placed each in the parent's mission.
        made.iter()
            .map(|i| {
                self.get_work_item(i.id)?
                    .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "proposal vanished"))
            })
            .collect()
    }

    /// Accept every proposal in `ids`, or none: one that is not a proposal
    /// waiting for a decision refuses the whole set.
    pub fn accept_proposals(&self, ids: &[i64]) -> Result<Vec<WorkItemRow>, IpcError> {
        if ids.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "accept_many needs item_ids",
            ));
        }
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            let wrote = self.conn.execute(
                "UPDATE work_items SET proposal_state = 'accepted', updated_at = ?1 \
                  WHERE id = ?2 AND origin = 'proposed' AND proposal_state = 'proposed'",
                rusqlite::params![now, id],
            )? == 1;
            if !wrote {
                return Err(match self.get_work_item(*id)? {
                    None => IpcError::new(codes::E_NOTFOUND, format!("work item {id} not found")),
                    Some(_) => IpcError::new(
                        codes::E_INVALID,
                        format!("work item {id} is not a proposal waiting for a decision"),
                    ),
                });
            }
        }
        tx.commit()?;
        self.emit_decided(ids)
    }

    /// Take an accept back: every item in `ids` returns to `proposed`, or
    /// none does. Only within [`ACCEPT_UNDO_SECS`] of the accept, and only
    /// for an item nothing has touched since — no session linked to it, no
    /// run of it, no subtask under it, its status still `todo`.
    pub fn undo_accept(&self, ids: &[i64]) -> Result<Vec<WorkItemRow>, IpcError> {
        if ids.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "undo_accept needs item_ids",
            ));
        }
        let since = now_unix() - ACCEPT_UNDO_SECS;
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            let item = self.get_work_item(*id)?.ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("work item {id} not found"))
            })?;
            if item.proposal_state.as_deref() != Some("accepted") {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("work item {id} is not an accepted proposal"),
                ));
            }
            let touched: bool = self.conn.query_row(
                "SELECT ?2 < ?3 OR ?4 <> 'todo' \
                    OR EXISTS(SELECT 1 FROM work_links WHERE item_id = ?1) \
                    OR EXISTS(SELECT 1 FROM tasks WHERE work_item_id = ?1) \
                    OR EXISTS(SELECT 1 FROM work_items WHERE parent_id = ?1)",
                rusqlite::params![id, item.updated_at, since, item.status_category],
                |r| r.get(0),
            )?;
            if touched {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "work item {id} was accepted more than {} minutes ago or has been \
                         worked on since; reject it instead",
                        ACCEPT_UNDO_SECS / 60
                    ),
                ));
            }
            self.conn.execute(
                "UPDATE work_items SET proposal_state = 'proposed', updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now_unix(), id],
            )?;
        }
        tx.commit()?;
        self.emit_decided(ids)
    }

    fn emit_decided(&self, ids: &[i64]) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            self.emit_work_item(
                *id,
                SessionChange {
                    primary: false,
                    suggested: false,
                    rejected: false,
                },
            )?;
            if let Some(row) = self.get_work_item(*id)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// The mission a new proposal under `parent_id` joins, after checking
    /// it has room for one more.
    pub(in crate::store) fn mission_for_new_proposal(
        &self,
        parent_id: i64,
    ) -> Result<Option<i64>, IpcError> {
        let mission = self.item_mission(parent_id)?;
        if let Some(m) = mission {
            self.check_mission_room(m, 1)?;
        }
        Ok(mission)
    }
}

#[cfg(test)]
mod tests;
