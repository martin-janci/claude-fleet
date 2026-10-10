//! Missions on the MCP surface and the desktop (orchestration O1, design
//! 2026-10-07 §4.1, §9). "Mission" in the UI, `orchestration_projects` in
//! the store.
//!
//! * Reads join `work`: `missions` (the ones this caller may read) and
//!   `mission` (one, with its member items, its latest events and the
//!   derived loop phase).
//! * Writes join `work_link`: `mission_save` (create, or change with
//!   `expected_version`), `mission_state`, `mission_repo`, `mission_item`,
//!   `mission_delete`. A per-host token is refused at the tool layer: a
//!   session does not run missions, a person does.
//!
//! **Who may read a mission:** the org boundary first, for everyone
//! (`OrgScope::sees_org` of the mission's org); then its owner, the hub's
//! own reader, the one person of a single-person hub for an unowned
//! mission ([`ViewScope::may_own_person_row`]), and a live member of the
//! mission's org. **Who may change it:** the same, except that of the org's
//! members only an admin may. Anyone else gets exactly the answer of an id
//! that does not exist (`orgs::not_found`).

use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::view_scope::ViewScope;
use crate::store::{
    MissionEventRow, MissionPatch, MissionRow, NewMission, PullRequestRow, Store, WorkItemRow,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Events a `mission` read carries; older ones are paged with
/// `before_event`.
pub const MISSION_EVENTS_PAGE: usize = 50;

/// `work_link { action: mission_save, mission }`: the fields a person
/// writes. On a create, `name` and `goal` are required and `org_id`
/// places the mission (0 or absent: none); on a change every field is
/// optional, an empty `non_goals` clears it, and `org_id` is refused (a
/// mission does not move between orgs in O1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_goals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<Vec<String>>,
    /// finite | continuous
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 0..=3
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// What it asks of its loop (O4); every field optional, defaults kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<crate::store::MissionPolicy>,
}

/// `work { action: mission, mission_id }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionDetail {
    pub mission: MissionRow,
    /// The member items this caller may see, root first.
    #[serde(default)]
    pub items: Vec<WorkItemRow>,
    /// Newest first, at most [`MISSION_EVENTS_PAGE`].
    #[serde(default)]
    pub events: Vec<MissionEventRow>,
    /// The loop's phase, derived and never stored (§4.5): `running` (a
    /// member has an open run), `blocked` (nothing is ready and something
    /// failed or waits outside) or `waiting`, for an active mission only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The dependency graph over `items`: each one's derived state and
    /// wave (orchestration O2).
    #[serde(default)]
    pub graph: super::graph::MissionGraph,
    /// Whether this caller may change it: the UI's buttons, not a fence.
    #[serde(default)]
    pub may_change: bool,
    /// Its loop (orchestration O4–O6): the next steps, the confirm queue,
    /// the autonomy that applies and what its workers spent. `None` for a
    /// draft or a finished mission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<super::orchestrate::MissionPlan>,
    /// What a finished mission leaves (Orbit Fleet G3.7): its live sessions
    /// and its pull requests. Absent unless the mission is finished, and
    /// from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<MissionFinish>,
}

/// A finished mission's leftovers, as far as the caller may see them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionFinish {
    /// The sessions still running on its member items: what "Archive N
    /// sessions" ends. Each is ended through the session's own Clean up
    /// (`discard_kill_session`, or the agent's safe remove when its tree is
    /// dirty), so its person fence is the session's.
    #[serde(default)]
    pub sessions: Vec<FinishSession>,
    /// The pull requests its work opened, newest change first.
    #[serde(default)]
    pub prs: Vec<PullRequestRow>,
}

/// One live session of a finished mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishSession {
    pub session_id: i64,
    /// The member item it works on.
    pub item_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub kind: String,
    /// Its own worktree's size in kB, from the host probe's last
    /// measurement (G1.9): what archiving it frees. Absent until measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_kb: Option<i64>,
}

fn not_found(id: i64) -> IpcError {
    orgs::not_found("mission", id)
}

/// The org role `scope`'s person holds in `org`, if both exist.
fn role_in(s: &Store, scope: &ViewScope, org: Option<i64>) -> Result<Option<String>, IpcError> {
    match (org, scope.person) {
        (Some(o), Some(p)) => s.org_role(o, p),
        _ => Ok(None),
    }
}

/// May `scope` read `m`?
pub fn sees_mission(s: &Store, scope: &ViewScope, m: &MissionRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(m.org_id, m.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(m.org_id) && role_in(s, scope, m.org_id)?.is_some())
}

/// May `scope` change `m`?
pub fn may_change_mission(s: &Store, scope: &ViewScope, m: &MissionRow) -> Result<bool, IpcError> {
    if scope.may_own_person_row(m.org_id, m.owner_person_id) {
        return Ok(true);
    }
    Ok(scope.org.sees_org(m.org_id)
        && role_in(s, scope, m.org_id)?.as_deref() == Some(crate::store::ROLE_ADMIN))
}

/// The mission, if `scope` may read it.
pub(super) fn visible(s: &Store, scope: &ViewScope, id: i64) -> Result<MissionRow, IpcError> {
    match s.get_mission(id)? {
        Some(m) if sees_mission(s, scope, &m)? => Ok(m),
        _ => Err(not_found(id)),
    }
}

/// The mission, if `scope` may change it. One it may read but not change
/// is `E_FORBIDDEN`; one it may not read is unknown.
pub(super) fn changeable(s: &Store, scope: &ViewScope, id: i64) -> Result<MissionRow, IpcError> {
    let m = visible(s, scope, id)?;
    if may_change_mission(s, scope, &m)? {
        Ok(m)
    } else {
        Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{} is its owner's; an admin of its organisation may change it too",
                m.name
            ),
        ))
    }
}

/// The work item, if `scope` may see it.
fn visible_item(s: &Store, scope: &ViewScope, id: i64) -> Result<WorkItemRow, IpcError> {
    match s.get_work_item(id)? {
        Some(i) if crate::service::trackers::tickets::item_visible(&scope.org, s, &i)? => Ok(i),
        _ => Err(orgs::not_found("work item", id)),
    }
}

/// Who an event says acted: the person, else the hub itself.
pub(super) fn actor(scope: &ViewScope) -> String {
    match scope.person {
        Some(p) => format!("person:{p}"),
        None => "fleet".into(),
    }
}

pub(super) fn mission_id(args: &WorkLinkArgs) -> Result<i64, IpcError> {
    args.mission_id.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{} needs mission_id", args.action),
        )
    })
}

/// `work { action: missions }`.
pub fn missions(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<MissionRow>, IpcError> {
    let s = lock(store)?;
    let now = crate::store::now_unix();
    let mut out = Vec::new();
    for mut m in s.list_missions()? {
        if sees_mission(&s, scope, &m)? {
            super::orchestrate::fill_spend(&s, &mut m, now)?;
            out.push(m);
        }
    }
    Ok(out)
}

/// `work { action: mission, mission_id, before_event? }`.
pub fn mission(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    before_event: Option<i64>,
) -> Result<MissionDetail, IpcError> {
    let s = lock(store)?;
    let mut mission = visible(&s, scope, id)?;
    super::orchestrate::fill_spend(&s, &mut mission, crate::store::now_unix())?;
    let mut items = Vec::new();
    for i in s.mission_items(id)? {
        if crate::service::trackers::tickets::item_visible(&scope.org, &s, &i)? {
            items.push(i);
        }
    }
    let graph = super::graph::build(&s, scope, &items)?;
    let phase = (mission.state == "active").then(|| graph.phase().to_string());
    let events = s.mission_events(id, before_event, MISSION_EVENTS_PAGE)?;
    let may_change = may_change_mission(&s, scope, &mission)?;
    let plan = super::orchestrate::plan_for(&s, &mission)?;
    let finish = if crate::store::MISSION_FINAL_STATES.contains(&mission.state.as_str()) {
        Some(finish(&s, scope, id)?)
    } else {
        None
    };
    Ok(MissionDetail {
        mission,
        items,
        events,
        phase,
        graph,
        may_change,
        plan,
        finish,
    })
}

/// A finished mission's live sessions and pull requests, each behind the
/// session's own fence: a session the caller may not see is not listed, and
/// a PR is listed only where `prs` would list it.
fn finish(s: &Store, scope: &ViewScope, id: i64) -> Result<MissionFinish, IpcError> {
    let mut sessions = Vec::new();
    for (sid, item_id) in s.mission_live_sessions(id)? {
        let Some(row) = s.get_session_by_id(sid)? else {
            continue;
        };
        if row.status != "running" || !scope.sees_session_row(&row).is_visible() {
            continue;
        }
        let worktree_kb = match row.worktree_id {
            Some(wt) => s.worktree_path(wt)?.and_then(|path| {
                crate::service::sessions::worktree_sizes::size_kb(&row.host_alias, &path)
                    .map(|(kb, _)| kb)
            }),
            None => None,
        };
        sessions.push(FinishSession {
            session_id: row.id,
            item_id,
            host_alias: row.host_alias,
            tmux_name: row.tmux_name,
            kind: row.kind,
            worktree_kb,
        });
    }
    let mut prs = Vec::new();
    for url in s.mission_pr_urls(id)? {
        if let Some(pr) = s.pull_request_by_url(&url)? {
            if crate::service::prs::visible(s, scope, &pr)? {
                prs.push(pr);
            }
        }
    }
    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));
    Ok(MissionFinish { sessions, prs })
}

/// `work_link { action: mission_save, mission, mission_id?, item_id?,
/// expected_version? }`: without `mission_id` a new draft, rooted at
/// `item_id` or at a new native task of the mission's name; with it, a
/// change.
pub fn save(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionRow, IpcError> {
    let input = args
        .mission
        .as_ref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "mission_save needs mission"))?;
    match args.mission_id {
        Some(id) => update(args, input, store, scope, id),
        None => create(args, input, store, scope),
    }
}

fn create(
    args: &WorkLinkArgs,
    input: &MissionInput,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionRow, IpcError> {
    let name = input
        .name
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "a new mission needs name"))?;
    let goal = input
        .goal
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "a new mission needs goal"))?;
    let s = lock(store)?;
    // The owner is the caller's person; the hub's own reader (the
    // standalone desktop) writes the hub's personal owner. A caller that
    // proves no person owns nothing.
    let owner = match scope.person {
        Some(p) => Some(p),
        None if scope.is_internal() => s.personal_owner_id()?,
        None => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a mission is a person's; this caller proves no person",
            ))
        }
    };
    let org = input.org_id.filter(|o| *o != 0);
    if !scope.org.sees_org(org) {
        return Err(match org {
            Some(o) => orgs::not_found("organisation", o),
            None => IpcError::new(
                codes::E_FORBIDDEN,
                "this caller may not create unassigned work; name its organisation",
            ),
        });
    }
    let root = match args.item_id {
        Some(item) => {
            visible_item(&s, scope, item)?;
            Some(item)
        }
        // This is the org boundary, not a privacy fence: the new root is a
        // standalone native task, which has no links and no sessions, so
        // there is no row and no person to fence — the refusal is about
        // authority to add top-level work, `local::create_task`'s rule.
        None if !scope.org.is_all() => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "root a mission at a task you can see (item_id); a new root task needs \
                 an unscoped caller, as any standalone task does",
            ))
        }
        None => None,
    };
    let new = NewMission {
        org_id: org,
        owner_person_id: owner,
        root_item_id: root,
        name,
        goal,
        non_goals: input.non_goals.as_deref(),
        done_when: input.done_when.as_deref().unwrap_or_default(),
        mode: input.mode.as_deref(),
        level: input.level,
    };
    // Everything a create can refuse is refused before a new root task is
    // made for it, so a refusal leaves nothing behind.
    s.check_new_mission(&new)?;
    let root_item_id = match root {
        Some(r) => r,
        None => {
            s.create_native_item(&crate::store::NativeItem {
                title: name,
                parent_id: None,
                project_id: None,
                notes: Some(goal),
            })?
            .id
        }
    };
    s.create_mission(
        &NewMission {
            root_item_id: Some(root_item_id),
            ..new
        },
        &actor(scope),
    )
}

fn update(
    args: &WorkLinkArgs,
    input: &MissionInput,
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
) -> Result<MissionRow, IpcError> {
    if input.org_id.is_some() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a mission keeps the organisation it was created in",
        ));
    }
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.update_mission(
        id,
        args.expected_version.filter(|v| *v != 0),
        &MissionPatch {
            name: input.name.clone(),
            goal: input.goal.clone(),
            non_goals: input
                .non_goals
                .as_ref()
                .map(|n| (!n.trim().is_empty()).then(|| n.clone())),
            done_when: input.done_when.clone(),
            mode: input.mode.clone(),
            level: input.level,
            policy: input.policy.clone(),
        },
        &actor(scope),
    )
}

/// `work_link { action: mission_state, mission_id, status, expected_version? }`.
pub fn set_state(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionRow, IpcError> {
    let id = mission_id(args)?;
    let to = args.status.as_deref().ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            "mission_state needs status: active | paused (reopens a finished one) | completed | failed | cancelled",
        )
    })?;
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.set_mission_state(
        id,
        args.expected_version.filter(|v| *v != 0),
        to,
        &actor(scope),
    )
}

/// `work_link { action: mission_repo, mission_id, project_id, role?, on? }`.
pub fn repo(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionRow, IpcError> {
    let id = mission_id(args)?;
    let project = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "mission_repo needs project_id"))?;
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    s.set_mission_repo(
        id,
        project,
        args.role.as_deref(),
        args.on.unwrap_or(true),
        &actor(scope),
    )
}

/// `work_link { action: mission_item, mission_id, item_id, on? }`. The
/// person fence on the item's own sessions is the tool layer's
/// (`require_drive_on_item_sessions`), as for a sprint.
pub fn item(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionRow, IpcError> {
    let id = mission_id(args)?;
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "mission_item needs item_id"))?;
    let s = lock(store)?;
    let m = changeable(&s, scope, id)?;
    visible_item(&s, scope, item)?;
    if let (Some(mo), Some(io)) = (m.org_id, s.item_org(item)?) {
        if mo != io {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "work item {item} belongs to another organisation than {}",
                    m.name
                ),
            ));
        }
    }
    s.set_mission_item(id, item, args.on.unwrap_or(true), &actor(scope))
}

/// What `mission_delete` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionDeleted {
    pub removed: i64,
}

/// `work_link { action: mission_delete, mission_id }`: a draft or a
/// finished mission; its items stay.
pub fn delete(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<MissionDeleted, IpcError> {
    let id = mission_id(args)?;
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    if !s.delete_mission(id)? {
        return Err(not_found(id));
    }
    Ok(MissionDeleted { removed: id })
}

#[cfg(test)]
mod tests;
