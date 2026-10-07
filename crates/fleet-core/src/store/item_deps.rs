//! The dependency graph between work items and a person's hold
//! (orchestration O2, design 2026-10-07 §4.3, migration 113).
//!
//! An edge says `item_id` waits for `depends_on`. Cycles are refused here,
//! in the transaction that would close one, by a walk over the edges; an
//! edge joins two items of the same org only. READY and BLOCKED are derived
//! on read (`service::work::mission_graph`) and never stored.
//!
//! An edge or a hold on a mission's member is logged in that mission's
//! event log (`dep_added`, `dep_removed`, `held`, `released`).

use super::orchestration::NewMissionEvent;
use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

/// Who may draw an edge.
pub const DEP_SOURCES: [&str; 3] = ["person", "planner", "proposal"];

/// The most items a cycle check walks before it refuses: a mission holds
/// 30, and an edge may reach a few items outside it.
pub const DEP_WALK_CAP: usize = 1000;

/// One edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDepRow {
    pub item_id: i64,
    pub depends_on: i64,
    pub kind: String,
    pub source: String,
    pub created_at: i64,
}

impl Store {
    /// Refuse an edge that would close a cycle or cross an org; `Ok(())`
    /// when it may be drawn.
    pub fn check_item_dep(&self, item_id: i64, depends_on: i64) -> Result<(), IpcError> {
        if item_id == depends_on {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a work item cannot wait for itself",
            ));
        }
        for id in [item_id, depends_on] {
            if self.get_work_item(id)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("work item {id} not found"),
                ));
            }
        }
        if self.item_org(item_id)? != self.item_org(depends_on)? {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!("work items {item_id} and {depends_on} belong to different organisations"),
            ));
        }
        // Would `depends_on` reach `item_id` through the edges already
        // there? Then the new edge closes a cycle.
        let mut seen: HashSet<i64> = HashSet::new();
        let mut queue: VecDeque<i64> = VecDeque::from([depends_on]);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT depends_on FROM work_item_deps WHERE item_id = ?1")?;
        while let Some(at) = queue.pop_front() {
            if at == item_id {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "work item {depends_on} already waits for {item_id}: the edge would \
                         close a cycle"
                    ),
                ));
            }
            if !seen.insert(at) {
                continue;
            }
            if seen.len() > DEP_WALK_CAP {
                return Err(IpcError::new(
                    codes::E_LIMIT,
                    format!("the dependency graph behind {depends_on} is too large to check"),
                ));
            }
            let next = stmt
                .query_map([at], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            queue.extend(next);
        }
        Ok(())
    }

    /// Draw `item_id → depends_on`. `Ok(false)`: it was already there.
    pub fn add_item_dep(
        &self,
        item_id: i64,
        depends_on: i64,
        source: &str,
        actor: &str,
    ) -> Result<bool, IpcError> {
        if !DEP_SOURCES.contains(&source) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("source is one of {}", DEP_SOURCES.join(", ")),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        self.check_item_dep(item_id, depends_on)?;
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO work_item_deps (item_id, depends_on, kind, source, created_at) \
             VALUES (?1, ?2, 'blocks', ?3, ?4)",
            rusqlite::params![item_id, depends_on, source, now_unix()],
        )? == 1;
        if added {
            self.log_item_event(item_id, "dep_added", actor, Some(depends_on))?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// Erase `item_id → depends_on`. `Ok(false)`: there was none.
    pub fn remove_item_dep(
        &self,
        item_id: i64,
        depends_on: i64,
        actor: &str,
    ) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let removed = self.conn.execute(
            "DELETE FROM work_item_deps WHERE item_id = ?1 AND depends_on = ?2",
            rusqlite::params![item_id, depends_on],
        )? == 1;
        if removed {
            self.log_item_event(item_id, "dep_removed", actor, Some(depends_on))?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Every edge out of any of `items` (what they wait for).
    pub fn item_deps(&self, items: &[i64]) -> Result<Vec<ItemDepRow>, IpcError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let list = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
        let mut stmt = self.conn.prepare(
            "SELECT item_id, depends_on, kind, source, created_at FROM work_item_deps \
             WHERE item_id IN (SELECT value FROM json_each(?1)) \
             ORDER BY item_id, depends_on",
        )?;
        let rows = stmt.query_map([list], |r| {
            Ok(ItemDepRow {
                item_id: r.get(0)?,
                depends_on: r.get(1)?,
                kind: r.get(2)?,
                source: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Put an item on hold, or release it. `Ok(false)`: nothing changed.
    pub fn set_item_hold(&self, item_id: i64, on: bool, actor: &str) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = if on {
            self.conn.execute(
                "UPDATE work_items SET held_at = ?1, updated_at = ?1 \
                 WHERE id = ?2 AND held_at IS NULL",
                rusqlite::params![now_unix(), item_id],
            )?
        } else {
            self.conn.execute(
                "UPDATE work_items SET held_at = NULL, updated_at = ?1 \
                 WHERE id = ?2 AND held_at IS NOT NULL",
                rusqlite::params![now_unix(), item_id],
            )?
        } == 1;
        if !changed && self.get_work_item(item_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {item_id} not found"),
            ));
        }
        if changed {
            self.log_item_event(item_id, if on { "held" } else { "released" }, actor, None)?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Log `kind` about `item_id` in its mission's log, when it has one.
    /// Runs inside the caller's transaction.
    fn log_item_event(
        &self,
        item_id: i64,
        kind: &str,
        actor: &str,
        depends_on: Option<i64>,
    ) -> Result<(), IpcError> {
        let mission: Option<i64> = self
            .conn
            .query_row(
                "SELECT orchestration_project_id FROM work_items WHERE id = ?1",
                [item_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(m) = mission {
            self.append_mission_event(
                m,
                &NewMissionEvent {
                    kind,
                    actor,
                    work_item_id: Some(item_id),
                    payload: depends_on.map(|d| serde_json::json!({ "depends_on": d })),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
