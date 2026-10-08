//! A mission's dependency graph, READY / BLOCKED and its waves
//! (orchestration O2, design 2026-10-07 §4.3), and the person's writes on
//! it: an edge, a hold, accepting a proposed plan and taking that back.
//!
//! Nothing here is stored but the edges and the hold: a node's state is
//! derived on every read from its status, its proposal, its runs and the
//! states of what it waits for, so it can never disagree with them.

use super::missions::{may_change_mission, sees_mission};
use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::trackers::tickets::item_visible;
use crate::service::view_scope::ViewScope;
use crate::store::{ItemDepRow, Store, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// What one node of the graph is doing now.
///
/// * `done` — its status is done (a tracker item: when the tracker says so).
/// * `proposed` / `rejected` — a proposal not accepted; never READY.
/// * `running` — a run of it is queued or running.
/// * `failed` — its latest run failed and nothing runs it now.
/// * `held` — a person stopped it.
/// * `verifying` — its implementation finished and it is not done yet: its
///   done_when is being checked (orchestration O3), or it waits to close.
/// * `doing` — in progress outside a run (a person or a session on it).
/// * `blocked` — it waits for something that failed, for something outside
///   the mission, or for something this caller cannot see.
/// * `waiting` — it waits for work that is still coming.
/// * `ready` — none of the above: it may start.
pub const NODE_STATES: [&str; 11] = [
    "done",
    "proposed",
    "rejected",
    "running",
    "failed",
    "verifying",
    "held",
    "doing",
    "blocked",
    "waiting",
    "ready",
];

/// One node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub item_id: i64,
    pub state: String,
    /// 1 for an item that waits for no other member; else one more than the
    /// latest wave it waits for.
    pub wave: u32,
    /// Every item it waits for, met or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<i64>,
    /// The ones not done yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting_for: Vec<i64>,
    /// Its done_when answer (orchestration O3), absent without lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<super::verify::Verification>,
    /// Its latest attempt, when the caller may see that task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<AttemptBrief>,
}

/// A node's latest attempt, in brief: what the worker said and what git
/// showed (orchestration O3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptBrief {
    pub task_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    pub state: String,
    /// The worker's reported outcome; absent without a report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::store::TaskEvidence>,
}

/// Longest summary a brief carries.
const BRIEF_SUMMARY_MAX_CHARS: usize = 300;

fn brief(t: &crate::store::TaskRow) -> AttemptBrief {
    let summary = t
        .report
        .as_ref()
        .map(|r| r.summary.clone())
        .filter(|x| !x.is_empty())
        .or_else(|| t.result.clone())
        .map(|x| x.chars().take(BRIEF_SUMMARY_MAX_CHARS).collect());
    AttemptBrief {
        task_id: t.id,
        role: t.role.clone(),
        attempt: t.attempt,
        state: t.state.clone(),
        outcome: t.report.as_ref().map(|r| r.outcome.clone()),
        summary,
        error: t.error.clone(),
        evidence: t.evidence.clone(),
    }
}

/// An item outside the mission that a member waits for, as far as the
/// caller may see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutsideItem {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    pub status_category: String,
}

/// A mission's graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionGraph {
    /// In wave order, then by item id.
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    /// The highest wave.
    #[serde(default)]
    pub waves: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside: Vec<OutsideItem>,
}

impl MissionGraph {
    /// The derived loop phase of an active mission (§4.5), minus
    /// `planning` and `needs_input`, which need the loop (O5).
    pub fn phase(&self) -> &'static str {
        let has = |s: &str| self.nodes.iter().any(|n| n.state == s);
        if has("running") {
            "running"
        } else if !has("ready") && (has("blocked") || has("failed")) {
            "blocked"
        } else {
            "waiting"
        }
    }
}

/// The graph over `items` (the members the caller may see), reading edges,
/// runs and outside items from `s`.
pub fn build(
    s: &Store,
    scope: &ViewScope,
    items: &[WorkItemRow],
) -> Result<MissionGraph, IpcError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let members: HashMap<i64, &WorkItemRow> = items.iter().map(|i| (i.id, i)).collect();
    let edges = s.item_deps(&ids)?;
    let mut deps: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for ItemDepRow {
        item_id,
        depends_on,
        ..
    } in &edges
    {
        deps.entry(*item_id).or_default().push(*depends_on);
    }
    // Outside items, read once each, kept only when the caller sees them.
    let mut outside: BTreeMap<i64, Option<WorkItemRow>> = BTreeMap::new();
    for d in edges.iter().map(|e| e.depends_on) {
        if members.contains_key(&d) || outside.contains_key(&d) {
            continue;
        }
        let row = match s.get_work_item(d)? {
            Some(r) if item_visible(&scope.org, s, &r)? => Some(r),
            _ => None,
        };
        outside.insert(d, row);
    }
    // Each member's own state, before what it waits for is weighed.
    let mut own: HashMap<i64, &'static str> = HashMap::new();
    let mut latest: HashMap<i64, AttemptBrief> = HashMap::new();
    for i in items {
        let tasks = s.tasks_for_item(i.id)?;
        if let Some(t) = tasks.first() {
            if crate::service::tasks::task_visible_in_scope(s, t, scope)? {
                latest.insert(i.id, brief(t));
            }
        }
        let open = tasks
            .iter()
            .any(|t| matches!(t.state.as_str(), "queued" | "running"));
        // The latest implementation attempt decides between failed and
        // implemented; a review or test run's verdict is its done_when's.
        let latest_impl = tasks
            .iter()
            .find(|t| matches!(t.role.as_deref(), None | Some("implement")));
        let failed =
            latest_impl.is_some_and(crate::service::work::orchestrate::steps::attempt_failed);
        let implemented = latest_impl.is_some_and(|t| t.state == "done") && !failed;
        let st = if i.status_category == "done" {
            "done"
        } else if i.proposal_state.as_deref() == Some("proposed") {
            "proposed"
        } else if i.proposal_state.as_deref() == Some("rejected") {
            "rejected"
        } else if open {
            "running"
        } else if failed {
            "failed"
        } else if implemented {
            "verifying"
        } else if i.held_at.is_some() {
            "held"
        } else if i.status_category == "in_progress" {
            "doing"
        } else {
            "ready"
        };
        own.insert(i.id, st);
    }
    let done = |id: i64| -> bool {
        match members.get(&id) {
            Some(m) => m.status_category == "done",
            None => outside
                .get(&id)
                .and_then(|r| r.as_ref())
                .is_some_and(|r| r.status_category == "done"),
        }
    };
    let mut nodes = Vec::with_capacity(items.len());
    for i in items {
        let on = deps.get(&i.id).cloned().unwrap_or_default();
        let waiting_for: Vec<i64> = on.iter().copied().filter(|d| !done(*d)).collect();
        let mut state = own[&i.id];
        if state == "ready" && !waiting_for.is_empty() {
            let stuck = waiting_for.iter().any(|d| match members.get(d) {
                Some(_) => matches!(own.get(d), Some(&"failed") | Some(&"rejected")),
                // Outside the mission, or hidden from this caller: nothing
                // this mission does will finish it.
                None => true,
            });
            state = if stuck { "blocked" } else { "waiting" };
        }
        nodes.push(GraphNode {
            item_id: i.id,
            state: state.to_string(),
            wave: 0,
            verification: super::verify::verification(s, i, now)?,
            attempt: latest.remove(&i.id),
            // A hidden outside item is not named to this caller.
            depends_on: on
                .iter()
                .copied()
                .filter(|d| members.contains_key(d) || matches!(outside.get(d), Some(Some(_))))
                .collect(),
            waiting_for: waiting_for
                .into_iter()
                .filter(|d| members.contains_key(d) || matches!(outside.get(d), Some(Some(_))))
                .collect(),
        });
    }
    // Waves: a topological depth over member edges only. The store refuses
    // a cycle, so the walk ends; the depth bound is a second net.
    let mut wave: HashMap<i64, u32> = HashMap::new();
    fn depth(
        id: i64,
        deps: &BTreeMap<i64, Vec<i64>>,
        members: &HashMap<i64, &WorkItemRow>,
        wave: &mut HashMap<i64, u32>,
        guard: usize,
    ) -> u32 {
        if let Some(w) = wave.get(&id) {
            return *w;
        }
        let w = if guard == 0 {
            1
        } else {
            1 + deps
                .get(&id)
                .map(|ds| {
                    ds.iter()
                        .filter(|d| members.contains_key(d))
                        .map(|d| depth(*d, deps, members, wave, guard - 1))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        };
        wave.insert(id, w);
        w
    }
    for n in &mut nodes {
        n.wave = depth(n.item_id, &deps, &members, &mut wave, members.len() + 1);
    }
    nodes.sort_by_key(|n| (n.wave, n.item_id));
    let waves = nodes.iter().map(|n| n.wave).max().unwrap_or(0);
    Ok(MissionGraph {
        nodes,
        waves,
        outside: outside
            .into_values()
            .flatten()
            .map(|r| OutsideItem {
                id: r.id,
                key: r.key,
                title: r.title,
                status_category: r.status_category,
            })
            .collect(),
    })
}

/// Who an event says acted.
pub(super) fn actor(scope: &ViewScope) -> String {
    match scope.person {
        Some(p) => format!("person:{p}"),
        None => "fleet".into(),
    }
}

/// The item, if `scope` may see it; else exactly an unknown id.
pub(super) fn visible_item(s: &Store, scope: &ViewScope, id: i64) -> Result<WorkItemRow, IpcError> {
    match s.get_work_item(id)? {
        Some(i) if item_visible(&scope.org, s, &i)? => Ok(i),
        _ => Err(orgs::not_found("work item", id)),
    }
}

/// Refuse a change to `item` when it belongs to a mission `scope` may not
/// change: one it cannot see answers as an unknown item, one it may only
/// read is `E_FORBIDDEN`.
pub(super) fn require_mission_change(
    s: &Store,
    scope: &ViewScope,
    item: i64,
) -> Result<(), IpcError> {
    let Some(m) = s.item_mission(item)? else {
        return Ok(());
    };
    let Some(m) = s.get_mission(m)? else {
        return Ok(());
    };
    if !sees_mission(s, scope, &m)? {
        return Err(orgs::not_found("work item", item));
    }
    if !may_change_mission(s, scope, &m)? {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "work item {item} is in {}, which its owner or an org admin changes",
                m.name
            ),
        ));
    }
    Ok(())
}

/// What `dep` and `hold` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphChange {
    pub item_id: i64,
    /// Whether anything changed.
    pub changed: bool,
}

/// `work_link { action: dep, item_id, depends_on, on? }`: `item_id` waits
/// for `depends_on` (`on: false` erases the edge).
pub fn dep(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<GraphChange, IpcError> {
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "dep needs item_id"))?;
    let on_id = args
        .depends_on
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "dep needs depends_on"))?;
    let s = lock(store)?;
    visible_item(&s, scope, item)?;
    visible_item(&s, scope, on_id)?;
    require_mission_change(&s, scope, item)?;
    let changed = if args.on.unwrap_or(true) {
        s.add_item_dep(item, on_id, "person", &actor(scope))?
    } else {
        s.remove_item_dep(item, on_id, &actor(scope))?
    };
    Ok(GraphChange {
        item_id: item,
        changed,
    })
}

/// `work_link { action: hold, item_id, on? }`: stop an item from becoming
/// READY, or release it.
pub fn hold(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<GraphChange, IpcError> {
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "hold needs item_id"))?;
    let s = lock(store)?;
    visible_item(&s, scope, item)?;
    require_mission_change(&s, scope, item)?;
    let changed = s.set_item_hold(item, args.on.unwrap_or(true), &actor(scope))?;
    Ok(GraphChange {
        item_id: item,
        changed,
    })
}

fn item_ids(args: &WorkLinkArgs) -> Result<&[i64], IpcError> {
    match args.item_ids.as_deref() {
        Some(ids) if !ids.is_empty() && ids.len() <= crate::store::PROPOSALS_OPEN_CAP * 3 => {
            Ok(ids)
        }
        Some(ids) if !ids.is_empty() => Err(IpcError::new(
            codes::E_LIMIT,
            format!(
                "at most {} item_ids at once",
                crate::store::PROPOSALS_OPEN_CAP * 3
            ),
        )),
        _ => Err(IpcError::new(
            codes::E_INVALID,
            format!("{} needs item_ids", args.action),
        )),
    }
}

/// The refusal a scoped caller gets for a person's decision, before any row
/// is reached.
pub(super) fn person_decides(scope: &ViewScope) -> Result<(), IpcError> {
    // This is the org boundary, not a privacy fence: deciding proposals is a
    // PERSON's act by design (an agent never accepts its own), the rule of
    // `local::decide`, so every scoped caller is refused outright and no row
    // is reached to fence. Each item's mission is fenced after it.
    if !scope.org.is_all() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a person decides proposals, from the desktop or the master token",
        ));
    }
    Ok(())
}

/// `work_link { action: accept_many, item_ids }`: accept a proposed plan
/// in one go, or none of it.
pub fn accept_many(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<Vec<WorkItemRow>, IpcError> {
    person_decides(scope)?;
    let ids = item_ids(args)?;
    let s = lock(store)?;
    for id in ids {
        require_mission_change(&s, scope, *id)?;
    }
    s.accept_proposals(ids)
}

/// `work_link { action: undo_accept, item_ids }`: take a recent accept
/// back (`store::ACCEPT_UNDO_SECS`).
pub fn undo_accept(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<Vec<WorkItemRow>, IpcError> {
    person_decides(scope)?;
    let ids = item_ids(args)?;
    let s = lock(store)?;
    for id in ids {
        require_mission_change(&s, scope, *id)?;
    }
    s.undo_accept(ids)
}

#[cfg(test)]
mod tests;
