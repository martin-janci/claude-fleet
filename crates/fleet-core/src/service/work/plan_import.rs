//! `work_link { action: mission_import, mission_id, plan }`: a plan's step
//! table read into a mission as its tasks and their edges, for the task
//! graph to draw (Missions › Graph).
//!
//! Each row is a step: its id (`3.1`), its title, the lane that owns it,
//! the steps it needs and, optionally, its status. A step becomes a local
//! task under the mission's root (top-level when it has none) titled
//! `"<step> <title>"`; its lane is its one assignee (a lane has one owner,
//! so the graph's "Assignee" lanes are the plan's lanes). Importing again
//! matches a member by that leading step id and brings it in line: title,
//! lane, status, and its edges to the other steps of the plan. Nothing is
//! ever removed but an edge between two steps of the plan that the plan no
//! longer draws.
//!
//! The whole table is checked before anything is written: the step ids, the
//! titles, a step needing itself and a cycle among the rows are refused up
//! front; a need naming no step is answered in `unknown_needs`, not refused.

use super::missions::{actor, changeable};
use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{ItemEdit, NativeItem, Store, WorkItemRow, STATUS_CATEGORIES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

/// Most rows one import takes.
pub const PLAN_IMPORT_MAX_ROWS: usize = 200;
/// Longest step id, in characters.
pub const PLAN_STEP_MAX_CHARS: usize = 24;

/// One row of a plan's step table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRow {
    /// The step's id, e.g. `3.1`: no whitespace.
    pub step: String,
    pub title: String,
    /// The lane that owns the step; absent leaves the task's assignees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// The step ids it waits for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// `todo` | `in_progress` | `done`; absent leaves a task's status (a
    /// new one starts `todo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanImport {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deps_added: usize,
    pub deps_removed: usize,
    /// Needs that name no step of the plan or of the mission, as
    /// `"<step> needs <id>"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_needs: Vec<String>,
}

/// The step id a member's title starts with, if it has one.
pub fn step_of(title: &str) -> Option<&str> {
    let first = title.split_whitespace().next()?;
    (title.len() > first.len() && first.chars().any(|c| c.is_ascii_digit())).then_some(first)
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg.into())
}

/// PURE: the table, checked. Answers the rows in order with their steps
/// trimmed, or the first thing wrong with it.
pub fn check_plan(rows: &[PlanRow]) -> Result<Vec<PlanRow>, IpcError> {
    if rows.is_empty() {
        return Err(invalid("a plan needs at least one row"));
    }
    if rows.len() > PLAN_IMPORT_MAX_ROWS {
        return Err(invalid(format!(
            "a plan has at most {PLAN_IMPORT_MAX_ROWS} rows; this one has {}",
            rows.len()
        )));
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut seen = BTreeSet::new();
    for (i, r) in rows.iter().enumerate() {
        let step = r.step.trim().to_string();
        if step.is_empty()
            || step.chars().count() > PLAN_STEP_MAX_CHARS
            || step.chars().any(|c| c.is_whitespace() || c.is_control())
            || !step.chars().any(|c| c.is_ascii_digit())
        {
            return Err(invalid(format!(
                "row {}: a step id is up to {PLAN_STEP_MAX_CHARS} characters with a digit \
                 and no space, like 3.1; got {:?}",
                i + 1,
                r.step
            )));
        }
        if !seen.insert(step.clone()) {
            return Err(invalid(format!("step {step} appears twice")));
        }
        if r.title.trim().is_empty() {
            return Err(invalid(format!("step {step} has no title")));
        }
        if let Some(s) = &r.status {
            if !STATUS_CATEGORIES.contains(&s.as_str()) {
                return Err(invalid(format!(
                    "step {step}: a status is one of {}",
                    STATUS_CATEGORIES.join(", ")
                )));
            }
        }
        let needs: Vec<String> = r
            .needs
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if needs.contains(&step) {
            return Err(invalid(format!("step {step} needs itself")));
        }
        out.push(PlanRow {
            step,
            title: r.title.trim().to_string(),
            lane: r
                .lane
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
            needs,
            status: r.status.clone(),
        });
    }
    if let Some(step) = cycle_in(&out) {
        return Err(invalid(format!(
            "the plan's needs go round in a circle through step {step}"
        )));
    }
    Ok(out)
}

/// PURE: a step on a cycle among the rows' needs, if there is one.
fn cycle_in(rows: &[PlanRow]) -> Option<String> {
    let needs: BTreeMap<&str, Vec<&str>> = rows
        .iter()
        .map(|r| {
            (
                r.step.as_str(),
                r.needs.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    // 0 unvisited, 1 on the stack, 2 done.
    let mut mark: HashMap<&str, u8> = HashMap::new();
    for &start in needs.keys() {
        if mark.get(start).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        mark.insert(start, 1);
        while let Some((node, i)) = stack.pop() {
            let next = needs.get(node).and_then(|n| n.get(i)).copied();
            match next {
                Some(n) => {
                    stack.push((node, i + 1));
                    if !needs.contains_key(n) {
                        continue;
                    }
                    match mark.get(n).copied().unwrap_or(0) {
                        1 => return Some(n.to_string()),
                        0 => {
                            mark.insert(n, 1);
                            stack.push((n, 0));
                        }
                        _ => {}
                    }
                }
                None => {
                    mark.insert(node, 2);
                }
            }
        }
    }
    None
}

/// `work_link { action: mission_import, mission_id, plan }`.
pub fn import(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<PlanImport, IpcError> {
    let id = super::missions::mission_id(args)?;
    let rows = check_plan(
        args.plan
            .as_deref()
            .ok_or_else(|| invalid("mission_import needs plan"))?,
    )?;
    let who = actor(scope);
    let s = lock(store)?;
    let m = changeable(&s, scope, id)?;
    let root = m.root_item_id;

    let members = s.mission_items(id)?;
    let mut by_step: HashMap<String, WorkItemRow> = HashMap::new();
    for it in &members {
        if Some(it.id) == root {
            continue;
        }
        if let Some(step) = step_of(&it.title) {
            by_step
                .entry(step.to_string())
                .or_insert_with(|| it.clone());
        }
    }
    let new_rows = rows
        .iter()
        .filter(|r| !by_step.contains_key(&r.step))
        .count();
    if new_rows > 0 {
        s.check_mission_room(id, new_rows)?;
    }

    let mut out = PlanImport::default();
    let mut ids: HashMap<String, i64> = HashMap::new();
    for r in &rows {
        let title = format!("{} {}", r.step, r.title);
        let lane = r.lane.clone().map(|l| vec![l]);
        let (row, created) = match by_step.get(&r.step) {
            Some(it) => (it.clone(), false),
            None => {
                let it = s.create_native_item(&NativeItem {
                    title: &title,
                    parent_id: root,
                    project_id: None,
                    notes: None,
                })?;
                s.set_mission_item(id, it.id, true, &who)?;
                (it, true)
            }
        };
        let mut changed = false;
        if row.source == "local" {
            let new_title = (row.title != title).then_some(title.as_str());
            let new_lane = lane.as_deref().filter(|l| *l != row.assignees.as_slice());
            if new_title.is_some() || new_lane.is_some() {
                s.edit_local_item(
                    row.id,
                    &ItemEdit {
                        title: new_title,
                        notes: None,
                        assignees: new_lane,
                        due_at: None,
                    },
                )?;
                changed = true;
            }
            if let Some(st) = r.status.as_deref() {
                if st != row.status_category {
                    s.set_item_status(row.id, st)?;
                    changed = true;
                }
            }
        }
        if created {
            out.created += 1;
        } else if changed {
            out.updated += 1;
        } else {
            out.unchanged += 1;
        }
        ids.insert(r.step.clone(), row.id);
    }
    // A need may name a member the table leaves out.
    for (step, it) in &by_step {
        ids.entry(step.clone()).or_insert(it.id);
    }

    let plan_items: BTreeSet<i64> = rows.iter().map(|r| ids[&r.step]).collect();
    let existing = s.item_deps(&plan_items.iter().copied().collect::<Vec<_>>())?;
    for r in &rows {
        let item = ids[&r.step];
        let mut want = BTreeSet::new();
        for n in &r.needs {
            match ids.get(n) {
                Some(&d) => {
                    want.insert(d);
                }
                None => out.unknown_needs.push(format!("{} needs {n}", r.step)),
            }
        }
        for &d in &want {
            if s.add_item_dep(item, d, "person", &who)? {
                out.deps_added += 1;
            }
        }
        for e in existing.iter().filter(|e| e.item_id == item) {
            if plan_items.contains(&e.depends_on)
                && !want.contains(&e.depends_on)
                && s.remove_item_dep(item, e.depends_on, &who)?
            {
                out.deps_removed += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
