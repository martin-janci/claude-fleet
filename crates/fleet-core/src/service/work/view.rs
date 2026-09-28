//! The Work view (work graph M14.1b, the reads): organisation → group →
//! task → every session, read as a projection of the one graph of items,
//! links and sessions (`docs/superpowers/specs/2026-09-27-work-view-design.md`).
//!
//! One read pass ([`Graph::load`]) takes every item, link, live session,
//! tracker, org, project, placement and rule under the store lock; the rest
//! is pure: which tasks and links the caller's [`OrgScope`] may see, where
//! each task sits (its org and its group, with their provenance), and the
//! filtered, sorted, keyset-paged answer.
//!
//! **Visibility** (the spec's security model, applied per task AND per
//! link, never from a filter):
//!
//! * a link is visible when its org is ([`OrgScope::sees_org`]) and so is
//!   its session: the live row ([`OrgScope::sees_row`]), or for a past
//!   session its snapshot's host and org — for a per-host token only its own
//!   host's past sessions (M2's fence);
//! * a task with an org of its own (a tracker's, or a local item's, M14) is
//!   visible when that org is; a per-host token additionally needs work of
//!   it on its own host (M3's fence); a task with no org of its own is
//!   visible to a scoped caller only through a visible link — or when it has
//!   no work at all yet (unassigned data: for a bound client only while its
//!   org's `bound_sees_unassigned` is on, D31).
//!
//! Rejected links are never tasks' sessions (only `task` lists them, as
//! decisions); ended suggestions and rejections are dropped altogether.

use super::resolve::Evidence;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::attention;
use crate::service::orgs::OrgScope;
use crate::store::{
    OrgRow, Placement, ProjectRow, SessionRow, Store, TrackerRow, ViewItem, ViewLink, WorkRule,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

/// Tasks per page by default, and at most.
pub const TREE_DEFAULT_LIMIT: usize = 50;
pub const TREE_MAX_LIMIT: usize = 200;
/// Sessions listed under each task by default, and at most.
pub const PER_TASK_DEFAULT: usize = 8;
pub const PER_TASK_MAX: usize = 50;
/// Review items per page.
pub const REVIEW_DEFAULT_LIMIT: usize = 50;
/// A tracker description in a task's detail, in characters.
pub const DESCRIPTION_MAX_CHARS: usize = 600;
/// A last-outcome summary, in characters.
pub const OUTCOME_MAX_CHARS: usize = 600;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// An id, or one of a few words (`"none"`, `"local"`, `"ref"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum IdOrWord {
    Id(i64),
    Word(String),
}

/// The Work view's filters: one object for a page, a saved view, the
/// desktop and the phone.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct WorkTreeFilters {
    /// Org id or "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<IdOrWord>,
    /// Tracker id, "local" or "ref".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<IdOrWord>,
    /// any|open|todo|in_progress|done
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Assigned to me.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mine: Option<bool>,
    /// any|active|past_only|none|suggested
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has: Option<String>,
    /// Something to review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<bool>,
    /// Key or title text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// One group id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Where a task sits and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    /// `label:<label>` (a person or a rule), `tracker:<id>:<container>`,
    /// `repo:<owner/repo>`, `key:<PREFIX>`, `none`.
    pub id: String,
    pub label: String,
    /// manual | rule | tracker | repo | key | none
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// What the tracker says, when the tracker has a say (kept even when a
    /// person or a rule placed the task elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_value: Option<String>,
    /// A person may place the task (always: placement is local).
    pub editable: bool,
}

/// One session under a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskLink {
    pub link_id: i64,
    pub link_version: i64,
    /// active | ended | suggested | rejected
    pub state: String,
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// One line: why this link exists.
    pub why: String,
    /// The link's evidence (`task` / `session_tasks` only; never in a tree
    /// page).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<serde_json::Value>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_status: Option<String>,
    pub needs_you: bool,
    pub archived: bool,
    pub resumable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// The task's org and the session's differ (a forced link).
    pub cross_org: bool,
    /// Other active tasks of this session the caller sees.
    pub other_tasks: u32,
    /// A person kept this conflict on purpose (`work_link { ack }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_ack_at: Option<i64>,
}

/// A task's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCounts {
    pub active: u32,
    pub ended: u32,
    pub suggested: u32,
}

/// One task of the Work view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkTask {
    /// `item:<id>` or `ref:<KEY>`.
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// tracker | local | ref
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The tracker's sync state: an outage is not "no sessions".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    pub unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    pub mine: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// tracker | item | sessions | none
    pub org_source: String,
    /// The org is a boundary (a tracker's or a local item's own).
    pub org_fenced: bool,
    /// An unfenced task whose sessions span orgs.
    pub org_mixed: bool,
    pub group: GroupRef,
    pub counts: TaskCounts,
    pub needs_you: bool,
    pub review: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    /// 0: no placement.
    pub placement_version: i64,
    pub sessions: Vec<TaskLink>,
    pub sessions_more: u32,
}

/// A section header of a page: one org's group and how many tasks match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    pub group: GroupRef,
    pub count: u32,
}

/// An org the caller sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgBrief {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A tracker the caller sees (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerBrief {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

/// `work { action: tree }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreePage {
    pub tasks: Vec<WorkTask>,
    pub groups: Vec<TreeGroup>,
    pub orgs: Vec<OrgBrief>,
    pub trackers: Vec<TrackerBrief>,
    pub total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub generated_at: i64,
}

/// A task's last known outcome: its newest past session, visible to the
/// caller, and that conversation's newest summary or note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastOutcome {
    pub at: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// summary | note | outcome
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_kind: Option<String>,
}

/// `work { action: task }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: WorkTask,
    /// Other ids that name this task (a bare key a sync bound to an item).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// The tracker's description (third-party text), capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<LastOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// Enabled rules whose conditions match the task (the first one places
    /// it unless a person did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<i64>,
}

/// The task one of a session's links points at, briefly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBrief {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_name: Option<String>,
}

/// One link of a session, with its task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTaskLink {
    #[serde(flatten)]
    pub link: TaskLink,
    pub task: TaskBrief,
}

/// `work { action: session_tasks }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTasks {
    pub session_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_link_id: Option<i64>,
    pub links: Vec<SessionTaskLink>,
}

/// One item of the review inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    /// `link:<id>`, or `session:<id>` for a session with no primary.
    pub review_id: String,
    /// suggestion | cross_org | unavailable | no_primary
    pub kind: String,
    pub session_id: i64,
    pub session_name: String,
    pub host: String,
    pub link_id: i64,
    pub link_version: i64,
    pub task: TaskBrief,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub why: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub preselected: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<ReviewAlternative>,
    pub created_at: i64,
}

/// Another suggestion of the same session, for *Change…*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAlternative {
    pub link_id: i64,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
}

/// `work { action: review }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewPage {
    pub items: Vec<ReviewItem>,
    pub total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// The graph
// ---------------------------------------------------------------------------

/// Everything one Work view read needs, taken in one pass under the lock.
pub(crate) struct Graph {
    pub(crate) now: i64,
    pub(crate) items: HashMap<i64, ViewItem>,
    pub(crate) links: Vec<ViewLink>,
    pub(crate) sessions: HashMap<i64, SessionRow>,
    pub(crate) trackers: HashMap<i64, TrackerRow>,
    pub(crate) orgs: Vec<OrgRow>,
    pub(crate) projects: HashMap<i64, ProjectRow>,
    pub(crate) placements: HashMap<String, Placement>,
    pub(crate) rules: Vec<WorkRule>,
}

impl Graph {
    pub(crate) fn load(s: &Store) -> Result<Graph, IpcError> {
        Ok(Graph {
            now: crate::service::catalog::now_secs(),
            items: s
                .work_view_items()?
                .into_iter()
                .map(|i| (i.item.id, i))
                .collect(),
            links: s.work_view_links()?,
            sessions: s
                .list_all_sessions()?
                .into_iter()
                .map(|r| (r.id, r))
                .collect(),
            trackers: s.list_trackers()?.into_iter().map(|t| (t.id, t)).collect(),
            orgs: s.list_orgs()?,
            projects: s.list_projects()?.into_iter().map(|p| (p.id, p)).collect(),
            placements: s
                .work_placements()?
                .into_iter()
                .map(|p| (p.task_id.clone(), p))
                .collect(),
            rules: s.work_rules()?,
        })
    }

    /// An item's own org: its tracker's, or a local item's (M14) —
    /// `Store::item_org` in memory (a test holds the two equal).
    pub(crate) fn item_org(&self, item: &ViewItem) -> Option<i64> {
        match item.item.tracker_id {
            Some(t) => self.trackers.get(&t).and_then(|t| t.org_id),
            None => item.own_org,
        }
    }

    /// The live session of a live link.
    fn session_of(&self, l: &ViewLink) -> Option<&SessionRow> {
        if l.link.ended_at.is_some() {
            return None;
        }
        self.row_of(l)
    }

    /// The session behind a link while its participant lives — also for a
    /// link that ended on a live session (a branch change), which has no
    /// snapshot: such a link is judged by the live row, never by an empty
    /// snapshot that would read as unassigned.
    fn row_of(&self, l: &ViewLink) -> Option<&SessionRow> {
        l.session_id.and_then(|id| self.sessions.get(&id))
    }

    /// `Store::link_org` in memory: the item's org, else the session's (the
    /// live row, else the snapshot's `snap_org_id`, which `map_link` leaves
    /// in `org_id`).
    fn link_org(&self, l: &ViewLink) -> Option<i64> {
        if let Some(org) = l
            .link
            .item_id
            .and_then(|i| self.items.get(&i))
            .and_then(|i| self.item_org(i))
        {
            return Some(org);
        }
        self.session_org(l)
    }

    /// The org of the session behind a link: the live row's, or the
    /// snapshot's.
    fn session_org(&self, l: &ViewLink) -> Option<i64> {
        match self.row_of(l) {
            Some(row) => row.org_id,
            None => l.link.org_id,
        }
    }

    fn task_id_of(l: &ViewLink) -> Option<String> {
        match (l.link.item_id, l.link.ref_key.as_deref()) {
            (Some(i), _) => Some(format!("item:{i}")),
            (None, Some(k)) => Some(format!("ref:{k}")),
            (None, None) => None,
        }
    }

    /// The wire state of a link, or `None` for one no view shows (an ended
    /// suggestion or rejection).
    fn state_of(&self, l: &ViewLink) -> Option<&'static str> {
        let live = l.link.ended_at.is_none() && self.session_of(l).is_some();
        match (l.link.state.as_str(), live) {
            ("confirmed", true) => Some("active"),
            ("confirmed", false) => Some("ended"),
            ("suggested", true) => Some("suggested"),
            ("rejected", true) => Some("rejected"),
            _ => None,
        }
    }

    /// May `scope` see this link (its org and its session)?
    fn link_visible(&self, scope: &OrgScope, l: &ViewLink) -> bool {
        if scope.is_all() {
            return true;
        }
        if !scope.sees_org(self.link_org(l)) {
            return false;
        }
        match self.row_of(l) {
            Some(row) => match scope {
                // M2's fence: a host's past work is its own host's.
                OrgScope::Host { alias, .. } if l.link.ended_at.is_some() => {
                    row.host_alias == *alias && scope.sees_row(row)
                }
                _ => scope.sees_row(row),
            },
            None => {
                let host = l.link.snap_host.as_deref().unwrap_or_default();
                match scope {
                    OrgScope::Host { alias, .. } => host == alias,
                    _ => scope.sees_session(host, l.link.org_id),
                }
            }
        }
    }

    fn is_mine(&self, item: &ViewItem) -> bool {
        let Some(t) = item.item.tracker_id.and_then(|t| self.trackers.get(&t)) else {
            return false;
        };
        t.config.account_id.is_some() && t.config.account_id == item.meta.assignee_id
    }

    fn repo_of(&self, l: &ViewLink) -> Option<String> {
        let pid = match self.session_of(l) {
            Some(row) => row.project_id,
            None => l.link.snap_project_id,
        }?;
        let p = self.projects.get(&pid)?;
        (p.owner != "local" && !p.owner.is_empty()).then(|| format!("{}/{}", p.owner, p.repo))
    }
}

// ---------------------------------------------------------------------------
// Building tasks
// ---------------------------------------------------------------------------

/// A task under construction: its links, the ones the caller sees, all
/// of them (for "no work at all yet"), and its item.
struct Built<'g> {
    task_id: String,
    item: Option<&'g ViewItem>,
    ref_key: Option<String>,
    /// Visible links with a wire state (rejected included; the tree drops
    /// them).
    visible: Vec<(&'g ViewLink, &'static str)>,
    /// Every link of any state, visible or not.
    any_links: usize,
    /// A per-host token's fence: work of it on its own host.
    on_own_host: bool,
}

fn build_tasks<'g>(g: &'g Graph, scope: &OrgScope) -> Vec<Built<'g>> {
    let mut by_task: BTreeMap<String, Built<'g>> = BTreeMap::new();
    for item in g.items.values() {
        let id = format!("item:{}", item.item.id);
        by_task.insert(
            id.clone(),
            Built {
                task_id: id,
                item: Some(item),
                ref_key: None,
                visible: Vec::new(),
                any_links: 0,
                on_own_host: false,
            },
        );
    }
    for l in &g.links {
        let Some(tid) = Graph::task_id_of(l) else {
            continue;
        };
        let state = g.state_of(l);
        if l.link.item_id.is_some() && !by_task.contains_key(&tid) {
            // An item gone from the view (a removed tracker's, unlinked):
            // nothing to hang the link on.
            continue;
        }
        if l.link.item_id.is_none() && !by_task.contains_key(&tid) {
            // A bare key is a task only through a live confirmed or
            // suggested link, or a past confirmed one.
            if !matches!(state, Some("active" | "ended" | "suggested")) {
                continue;
            }
            by_task.insert(
                tid.clone(),
                Built {
                    task_id: tid.clone(),
                    item: None,
                    ref_key: l.link.ref_key.clone(),
                    visible: Vec::new(),
                    any_links: 0,
                    on_own_host: false,
                },
            );
        }
        let Some(b) = by_task.get_mut(&tid) else {
            continue;
        };
        b.any_links += 1;
        let Some(state) = state else {
            continue;
        };
        if !g.link_visible(scope, l) {
            continue;
        }
        if let OrgScope::Host { alias, .. } = scope {
            let host = match g.row_of(l) {
                Some(row) => Some(row.host_alias.as_str()),
                None => l.link.snap_host.as_deref(),
            };
            if host == Some(alias.as_str()) && matches!(state, "active" | "ended") {
                b.on_own_host = true;
            }
        }
        b.visible.push((l, state));
    }
    by_task
        .into_values()
        .filter(|b| task_visible(g, scope, b))
        .collect()
}

fn task_visible(g: &Graph, scope: &OrgScope, b: &Built<'_>) -> bool {
    if scope.is_all() {
        return true;
    }
    let fence = b.item.and_then(|i| g.item_org(i));
    if fence.is_some() && !scope.sees_org(fence) {
        return false;
    }
    let shown = b.visible.iter().any(|(_, st)| *st != "rejected");
    match scope {
        OrgScope::All => true,
        // M3's fence: a host reads only work its own host did.
        OrgScope::Host { .. } => b.on_own_host,
        // A task with no work at all yet is unassigned data: seen only
        // while the org's `bound_sees_unassigned` is on (D31).
        OrgScope::Org { .. } => {
            fence.is_some() || shown || (b.any_links == 0 && scope.sees_org(None))
        }
    }
}

/// Why a link exists, in one line.
pub fn why_line(source: &str, evidence: &[serde_json::Value]) -> String {
    if let Some(e) = evidence
        .last()
        .and_then(|e| serde_json::from_value::<Evidence>(e.clone()).ok())
    {
        let what = match serde_json::to_value(e.signal)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .as_deref()
        {
            Some("branch") => "branch",
            Some("pr_head") => "PR branch",
            Some("pr_closing") => "PR closes",
            Some("pr_text") => "PR mentions",
            Some("trailer") => "commit trailer",
            Some("prompt_url") => "ticket URL in a prompt",
            Some("prompt_key") => "mentioned in a prompt",
            Some("prompt_issue") => "issue number in a prompt",
            Some("agent_inferred") => "Claude named it",
            _ => "seen",
        };
        let text: String = e.text.chars().take(60).collect();
        return format!("{what} {text} · {}", e.rule).trim().to_string();
    }
    match source {
        "manual" => "linked by a person".into(),
        "started" => "started for this task".into(),
        "agent" => "Claude linked it".into(),
        "resumed" => "resumed from past work".into(),
        "forked" => "copied when the session was forked".into(),
        "inherited" => "inherited from its parent session".into(),
        other => format!("linked ({other})"),
    }
}

fn strength_rank(s: Option<&str>) -> u8 {
    match s {
        Some("explicit") => 0,
        Some("strong") => 1,
        Some("inferred") => 2,
        _ => 3,
    }
}

fn state_rank(st: &str) -> u8 {
    match st {
        "active" => 0,
        "suggested" => 1,
        "ended" => 2,
        _ => 3,
    }
}

/// The group a task sits in, and why.
fn group_of(
    g: &Graph,
    task_id: &str,
    item: Option<&ViewItem>,
    key: Option<&str>,
    title: &str,
    repos: &[String],
) -> GroupRef {
    let tracker_value = item
        .filter(|i| i.item.tracker_id.is_some())
        .and_then(|i| i.containers.first().cloned());
    if let Some(label) = g
        .placements
        .get(task_id)
        .and_then(|p| p.group.clone())
        .filter(|l| !l.trim().is_empty())
    {
        return GroupRef {
            id: format!("label:{label}"),
            label,
            source: "manual".into(),
            rule_id: None,
            tracker_value,
            editable: true,
        };
    }
    if let Some(r) = g
        .rules
        .iter()
        .find(|r| r.enabled && rule_matches(r, item, key, title, repos))
    {
        return GroupRef {
            id: format!("label:{}", r.group),
            label: r.group.clone(),
            source: "rule".into(),
            rule_id: Some(r.id),
            tracker_value,
            editable: true,
        };
    }
    if let (Some(i), Some(c)) = (item, tracker_value.clone()) {
        let tid = i.item.tracker_id.unwrap_or_default();
        let provider = g.trackers.get(&tid).map(|t| t.provider.as_str());
        let label = if provider == Some("asana") {
            let tail: String = c
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("Asana project …{tail}")
        } else {
            c.clone()
        };
        return GroupRef {
            id: format!("tracker:{tid}:{c}"),
            label,
            source: "tracker".into(),
            rule_id: None,
            tracker_value: Some(c),
            editable: true,
        };
    }
    if let Some(repo) = repos.first() {
        return GroupRef {
            id: format!("repo:{repo}"),
            label: repo.clone(),
            source: "repo".into(),
            rule_id: None,
            tracker_value: None,
            editable: true,
        };
    }
    if let Some(prefix) = key.and_then(key_prefix) {
        return GroupRef {
            id: format!("key:{prefix}"),
            label: prefix,
            source: "key".into(),
            rule_id: None,
            tracker_value: None,
            editable: true,
        };
    }
    GroupRef {
        id: "none".into(),
        label: "No group".into(),
        source: "none".into(),
        rule_id: None,
        tracker_value: None,
        editable: true,
    }
}

/// `ABC` of `ABC-12`; `None` for anything else.
fn key_prefix(key: &str) -> Option<String> {
    let (p, n) = key.split_once('-')?;
    (!p.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .then(|| p.to_ascii_uppercase())
}

/// Does rule `r` match a task? Every condition it sets must.
pub(crate) fn rule_matches(
    r: &WorkRule,
    item: Option<&ViewItem>,
    key: Option<&str>,
    title: &str,
    repos: &[String],
) -> bool {
    let c = &r.conditions;
    if let Some(t) = c.tracker_id {
        if item.and_then(|i| i.item.tracker_id) != Some(t) {
            return false;
        }
    }
    if let Some(want) = &c.container {
        let ok = item.is_some_and(|i| i.containers.iter().any(|x| x.eq_ignore_ascii_case(want)));
        if !ok {
            return false;
        }
    }
    if let Some(p) = &c.key_prefix {
        if key.and_then(key_prefix).as_deref() != Some(p.to_ascii_uppercase().as_str()) {
            return false;
        }
    }
    if let Some(t) = &c.title_contains {
        if !title.to_lowercase().contains(&t.to_lowercase()) {
            return false;
        }
    }
    if let Some(repo) = &c.repo {
        if !repos.iter().any(|x| x.eq_ignore_ascii_case(repo)) {
            return false;
        }
    }
    true
}

/// The links a scoped caller sees of every live session, active only:
/// session id → the task ids (for `other_tasks`).
fn active_tasks_by_session(tasks: &[Built<'_>]) -> HashMap<i64, BTreeSet<String>> {
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    for b in tasks {
        for (l, st) in &b.visible {
            if *st == "active" {
                if let Some(sid) = l.session_id {
                    out.entry(sid).or_default().insert(b.task_id.clone());
                }
            }
        }
    }
    out
}

fn task_link(
    g: &Graph,
    l: &ViewLink,
    state: &str,
    task_id: &str,
    others: &HashMap<i64, BTreeSet<String>>,
    with_evidence: bool,
) -> TaskLink {
    let row = g.session_of(l);
    let fence = l
        .link
        .item_id
        .and_then(|i| g.items.get(&i))
        .and_then(|i| g.item_org(i));
    let session_org = g.session_org(l);
    let cross_org = matches!((fence, session_org), (Some(a), Some(b)) if a != b);
    TaskLink {
        link_id: l.link.id,
        link_version: l.version,
        state: state.to_string(),
        primary: state == "active" && l.link.is_primary,
        session_id: row.map(|r| r.id),
        name: match row {
            Some(r) => r
                .friendly_name
                .clone()
                .unwrap_or_else(|| r.tmux_name.clone()),
            None => l
                .link
                .snap_name
                .clone()
                .or_else(|| l.link.snap_tmux.clone())
                .unwrap_or_else(|| "past session".into()),
        },
        host: row
            .map(|r| r.host_alias.clone())
            .or_else(|| l.link.snap_host.clone()),
        source: l.link.source.clone(),
        strength: l.link.strength.clone(),
        rule: l.link.rule.clone(),
        why: why_line(&l.link.source, &l.link.evidence),
        evidence: if with_evidence {
            l.link.evidence.clone()
        } else {
            Vec::new()
        },
        created_at: l.link.created_at,
        decided_at: l.link.decided_at,
        ended_at: l.link.ended_at,
        end_reason: l.link.end_reason.clone(),
        claude_status: row.and_then(|r| r.claude_status.clone()),
        needs_you: row.is_some_and(|r| attention::needs_attention(r).is_some()),
        archived: l.archived_at.is_some(),
        resumable: l.link.resumable,
        branch: l.link.snap_branch.clone().filter(|_| row.is_none()),
        pr_url: row
            .and_then(|r| r.pr_url.clone())
            .or_else(|| l.link.snap_pr_url.clone()),
        cross_org,
        other_tasks: row.and_then(|r| others.get(&r.id)).map_or(0, |t| {
            t.iter().filter(|x| x.as_str() != task_id).count() as u32
        }),
        review_ack_at: l.review_ack_at,
    }
}

/// Is this visible link something the review inbox lists?
fn needs_review(g: &Graph, l: &ViewLink, state: &str) -> bool {
    match state {
        "suggested" => true,
        "active" if l.review_ack_at.is_none() => {
            let item = l.link.item_id.and_then(|i| g.items.get(&i));
            let unavailable = item.is_some_and(|i| i.item.unavailable_at.is_some());
            let fence = item.and_then(|i| g.item_org(i));
            let cross = matches!((fence, g.session_org(l)), (Some(a), Some(b)) if a != b);
            unavailable || cross
        }
        _ => false,
    }
}

fn to_task(
    g: &Graph,
    b: &Built<'_>,
    per_task: usize,
    others: &HashMap<i64, BTreeSet<String>>,
    with_evidence: bool,
    with_rejected: bool,
) -> WorkTask {
    let item = b.item;
    let key = item
        .and_then(|i| i.item.key.clone())
        .or_else(|| b.ref_key.clone());
    let title = item.map(|i| i.item.title.clone()).unwrap_or_default();
    let tracker = item
        .and_then(|i| i.item.tracker_id)
        .and_then(|t| g.trackers.get(&t));
    let mut links: Vec<(&ViewLink, &str)> = b
        .visible
        .iter()
        .filter(|(_, st)| with_rejected || *st != "rejected")
        .map(|(l, st)| (*l, *st))
        .collect();
    links.sort_by(|(a, sa), (b, sb)| {
        state_rank(sa)
            .cmp(&state_rank(sb))
            .then((!a.link.is_primary).cmp(&!b.link.is_primary))
            .then(
                strength_rank(a.link.strength.as_deref())
                    .cmp(&strength_rank(b.link.strength.as_deref())),
            )
            .then(
                b.link
                    .ended_at
                    .or(b.link.decided_at)
                    .unwrap_or(b.link.created_at)
                    .cmp(
                        &a.link
                            .ended_at
                            .or(a.link.decided_at)
                            .unwrap_or(a.link.created_at),
                    ),
            )
            .then(b.link.id.cmp(&a.link.id))
    });
    let mut counts = TaskCounts::default();
    let mut active_sessions = BTreeSet::new();
    let mut needs_you = false;
    let mut review = false;
    let mut last: Option<i64> = None;
    let mut repos: Vec<String> = Vec::new();
    let mut link_orgs: BTreeSet<Option<i64>> = BTreeSet::new();
    for (l, st) in &links {
        match *st {
            "active" => {
                if let Some(sid) = l.session_id {
                    active_sessions.insert(sid);
                }
            }
            "ended" => counts.ended += 1,
            "suggested" => counts.suggested += 1,
            _ => {}
        }
        if *st == "rejected" {
            continue;
        }
        let row = g.session_of(l);
        if *st == "active" && row.is_some_and(|r| attention::needs_attention(r).is_some()) {
            needs_you = true;
        }
        if needs_review(g, l, st) {
            review = true;
        }
        let at = row
            .map(|r| r.last_activity_at)
            .or(l.link.ended_at)
            .unwrap_or(l.link.decided_at.unwrap_or(l.link.created_at));
        last = Some(last.map_or(at, |x| x.max(at)));
        if let Some(r) = g.repo_of(l) {
            if !repos.contains(&r) {
                repos.push(r);
            }
        }
        if matches!(*st, "active" | "ended") {
            link_orgs.insert(g.session_org(l));
        }
    }
    counts.active = active_sessions.len() as u32;
    if let Some(i) = item {
        let ext = i.item.updated_ext.unwrap_or(i.item.updated_at);
        last = Some(last.map_or(ext, |x| x.max(ext)));
    }
    let fence = item.and_then(|i| g.item_org(i));
    let (org_id, org_source, org_fenced, org_mixed) = match fence {
        Some(o) => (
            Some(o),
            if item.is_some_and(|i| i.item.tracker_id.is_some()) {
                "tracker"
            } else {
                "item"
            },
            true,
            false,
        ),
        None => {
            let orgs: BTreeSet<i64> = link_orgs.iter().filter_map(|o| *o).collect();
            match orgs.len() {
                1 if !link_orgs.contains(&None) => {
                    (orgs.into_iter().next(), "sessions", false, false)
                }
                0 => (None, "none", false, false),
                _ => (None, "none", false, true),
            }
        }
    };
    let group = group_of(g, &b.task_id, item, key.as_deref(), &title, &repos);
    let total_links = links.len();
    let shown: Vec<TaskLink> = links
        .iter()
        .take(per_task)
        .map(|(l, st)| task_link(g, l, st, &b.task_id, others, with_evidence))
        .collect();
    WorkTask {
        task_id: b.task_id.clone(),
        item_id: item.map(|i| i.item.id),
        key,
        title,
        url: item.and_then(|i| i.item.url.clone()),
        kind: match item {
            Some(i) if i.item.tracker_id.is_some() => "tracker",
            Some(_) => "local",
            None => "ref",
        }
        .into(),
        tracker_id: item.and_then(|i| i.item.tracker_id),
        tracker_name: tracker.map(|t| t.name.clone()),
        provider: tracker.map(|t| t.provider.clone()),
        tracker_state: tracker.map(|t| t.state.clone()),
        status_category: item
            .map(|i| i.item.status_category.clone())
            .filter(|c| !c.is_empty()),
        status_name: item.and_then(|i| i.item.status_name.clone()),
        resolution: item.and_then(|i| i.item.resolution.clone()),
        unavailable: item.is_some_and(|i| i.item.unavailable_at.is_some()),
        unavailable_reason: item.and_then(|i| i.item.unavailable_reason.clone()),
        assignees: item.map(|i| i.item.assignees.clone()).unwrap_or_default(),
        mine: item.is_some_and(|i| g.is_mine(i)),
        org_id,
        org_source: org_source.into(),
        org_fenced,
        org_mixed,
        group,
        counts,
        needs_you,
        review,
        last_activity_at: last,
        repos,
        placement_version: g.placements.get(&b.task_id).map_or(0, |p| p.version),
        sessions_more: total_links.saturating_sub(shown.len()) as u32,
        sessions: shown,
    }
}

// ---------------------------------------------------------------------------
// Filters, order, cursor
// ---------------------------------------------------------------------------

fn bad(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

/// Validate filters before any work: a bad value is refused, never
/// silently ignored (a filter the client thinks applied must apply).
pub fn check_filters(f: &WorkTreeFilters) -> Result<(), IpcError> {
    if let Some(IdOrWord::Word(w)) = &f.org {
        if w != "none" {
            return Err(bad(format!(
                "filters.org is an org id or \"none\", not {w:?}"
            )));
        }
    }
    if let Some(IdOrWord::Word(w)) = &f.tracker {
        if !matches!(w.as_str(), "local" | "ref") {
            return Err(bad(format!(
                "filters.tracker is a tracker id, \"local\" or \"ref\", not {w:?}"
            )));
        }
    }
    if let Some(s) = f.status.as_deref() {
        if !matches!(s, "any" | "open" | "todo" | "in_progress" | "done") {
            return Err(bad(format!(
                "filters.status is any, open, todo, in_progress or done, not {s:?}"
            )));
        }
    }
    if let Some(h) = f.has.as_deref() {
        if !matches!(h, "any" | "active" | "past_only" | "none" | "suggested") {
            return Err(bad(format!(
                "filters.has is any, active, past_only, none or suggested, not {h:?}"
            )));
        }
    }
    if f.query.as_deref().is_some_and(|q| q.chars().count() > 200) {
        return Err(bad("filters.query is longer than 200 characters"));
    }
    Ok(())
}

fn matches_filters(t: &WorkTask, f: &WorkTreeFilters, with_group: bool) -> bool {
    match &f.org {
        Some(IdOrWord::Id(o)) if t.org_id != Some(*o) => return false,
        Some(IdOrWord::Word(_)) if t.org_id.is_some() => return false,
        _ => {}
    }
    match &f.tracker {
        Some(IdOrWord::Id(id)) if t.tracker_id != Some(*id) => return false,
        Some(IdOrWord::Word(w)) if w != &t.kind => return false,
        _ => {}
    }
    let status = t.status_category.as_deref();
    let ok = match f.status.as_deref() {
        None | Some("any") => true,
        Some("open") => status != Some("done"),
        Some(s) => status == Some(s),
    };
    if !ok {
        return false;
    }
    if f.mine == Some(true) && !t.mine {
        return false;
    }
    let c = t.counts;
    let ok = match f.has.as_deref() {
        None | Some("any") => true,
        Some("active") => c.active > 0,
        Some("past_only") => c.active == 0 && c.ended > 0,
        Some("none") => c.active == 0 && c.ended == 0,
        Some("suggested") => c.suggested > 0,
        Some(_) => true,
    };
    if !ok {
        return false;
    }
    if f.review == Some(true) && !t.review {
        return false;
    }
    if let Some(q) = f.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let q = q.to_lowercase();
        let hay = format!("{} {}", t.key.as_deref().unwrap_or_default(), t.title).to_lowercase();
        if !hay.contains(&q) {
            return false;
        }
    }
    if with_group {
        if let Some(gid) = f.group.as_deref() {
            if t.group.id != gid {
                return false;
            }
        }
    }
    true
}

/// A task's place in the order: named orgs by name then unassigned, groups
/// by label (`none` last), tasks needing a person first, then the latest
/// activity, then the id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct SortKey(u8, String, u8, String, u8, i64, String);

fn sort_key(t: &WorkTask, org_names: &HashMap<i64, String>) -> SortKey {
    let (org_rank, org_name) = match t.org_id.and_then(|o| org_names.get(&o)) {
        Some(n) => (0, n.to_lowercase()),
        None => (1, String::new()),
    };
    SortKey(
        org_rank,
        org_name,
        u8::from(t.group.source == "none"),
        t.group.label.to_lowercase(),
        u8::from(!t.needs_you),
        -t.last_activity_at.unwrap_or(0),
        t.task_id.clone(),
    )
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    /// The filters' hash: a cursor is valid only with the filters it was
    /// made under.
    f: String,
    k: serde_json::Value,
}

fn filters_hash(f: &WorkTreeFilters) -> String {
    use sha2::Digest;
    let raw = serde_json::to_string(f).unwrap_or_default();
    hex::encode(&sha2::Sha256::digest(raw.as_bytes())[..8])
}

fn encode_cursor<K: Serialize>(hash: String, key: &K) -> String {
    use base64::Engine as _;
    let c = Cursor {
        f: hash,
        k: serde_json::to_value(key).unwrap_or_default(),
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&c).unwrap_or_default())
}

fn decode_cursor<K: for<'de> Deserialize<'de>>(raw: &str, hash: &str) -> Result<K, IpcError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim())
        .map_err(|_| bad("cursor is not one this hub made"))?;
    let c: Cursor =
        serde_json::from_slice(&bytes).map_err(|_| bad("cursor is not one this hub made"))?;
    if c.f != hash {
        return Err(bad(
            "cursor was made for other filters; start again without a cursor",
        ));
    }
    serde_json::from_value(c.k).map_err(|_| bad("cursor is not one this hub made"))
}

// ---------------------------------------------------------------------------
// The reads
// ---------------------------------------------------------------------------

/// `work { action: tree }`'s arguments.
#[derive(Debug, Clone, Default)]
pub struct TreeArgs {
    pub filters: WorkTreeFilters,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub per_task: Option<usize>,
}

fn visible_orgs(g: &Graph, scope: &OrgScope) -> Vec<OrgBrief> {
    g.orgs
        .iter()
        .filter(|o| scope.sees_org(Some(o.id)))
        .map(|o| OrgBrief {
            id: o.id,
            name: o.name.clone(),
            color: o.color.clone(),
        })
        .collect()
}

fn visible_trackers(g: &Graph, scope: &OrgScope, tasks: &[WorkTask]) -> Vec<TrackerBrief> {
    let used: BTreeSet<i64> = tasks.iter().filter_map(|t| t.tracker_id).collect();
    let mut out: Vec<TrackerBrief> = g
        .trackers
        .values()
        .filter(|t| match scope {
            OrgScope::All => true,
            OrgScope::Org { .. } => scope.sees_org(t.org_id),
            // A host names only the trackers of the work it reads.
            OrgScope::Host { .. } => used.contains(&t.id),
        })
        .map(|t| TrackerBrief {
            id: t.id,
            name: t.name.clone(),
            provider: t.provider.clone(),
            state: t.state.clone(),
            org_id: t.org_id,
        })
        .collect();
    out.sort_by_key(|t| t.id);
    out
}

/// Every task the caller sees, built (sessions capped at `per_task`).
pub(crate) fn all_tasks(g: &Graph, scope: &OrgScope, per_task: usize) -> Vec<WorkTask> {
    let built = build_tasks(g, scope);
    let others = active_tasks_by_session(&built);
    built
        .iter()
        .map(|b| to_task(g, b, per_task, &others, false, false))
        .collect()
}

/// [`tree`] over a graph already loaded (tests and the scale budget).
pub(crate) fn tree_of(g: &Graph, scope: &OrgScope, args: &TreeArgs) -> Result<TreePage, IpcError> {
    check_filters(&args.filters)?;
    let limit = args
        .limit
        .unwrap_or(TREE_DEFAULT_LIMIT)
        .clamp(1, TREE_MAX_LIMIT);
    let per_task = args.per_task.unwrap_or(PER_TASK_DEFAULT).min(PER_TASK_MAX);
    let org_names: HashMap<i64, String> = g.orgs.iter().map(|o| (o.id, o.name.clone())).collect();
    let all = all_tasks(g, scope, per_task);
    // Section headers count every task under the filters but the group
    // one, so every section of the view has its header and count.
    let mut groups: BTreeMap<(Option<i64>, String), (GroupRef, u32, SortKey)> = BTreeMap::new();
    let mut matching: Vec<(SortKey, WorkTask)> = Vec::new();
    for t in all {
        if !matches_filters(&t, &args.filters, false) {
            continue;
        }
        let key = sort_key(&t, &org_names);
        let e = groups
            .entry((t.org_id, t.group.id.clone()))
            .or_insert_with(|| (t.group.clone(), 0, key.clone()));
        e.1 += 1;
        if key < e.2 {
            e.2 = key.clone();
        }
        if args
            .filters
            .group
            .as_deref()
            .is_none_or(|gid| gid == t.group.id)
        {
            matching.push((key, t));
        }
    }
    matching.sort_by(|a, b| a.0.cmp(&b.0));
    let hash = filters_hash(&args.filters);
    let start = match args.cursor.as_deref().filter(|c| !c.trim().is_empty()) {
        None => 0,
        Some(c) => {
            let after: SortKey = decode_cursor(c, &hash)?;
            matching.partition_point(|(k, _)| *k <= after)
        }
    };
    let total = matching.len() as u32;
    let page: Vec<(SortKey, WorkTask)> = matching.into_iter().skip(start).take(limit + 1).collect();
    let more = page.len() > limit;
    let page: Vec<(SortKey, WorkTask)> = page.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last().map(|(k, _)| encode_cursor(hash.clone(), k))
    } else {
        None
    };
    let tasks: Vec<WorkTask> = page.into_iter().map(|(_, t)| t).collect();
    let mut group_list: Vec<(SortKey, TreeGroup)> = groups
        .into_iter()
        .map(|((org, _), (group, count, key))| {
            (
                SortKey(key.0, key.1, key.2, key.3, 0, 0, String::new()),
                TreeGroup {
                    org_id: org,
                    org_name: org.and_then(|o| org_names.get(&o).cloned()),
                    group,
                    count,
                },
            )
        })
        .collect();
    group_list.sort_by(|a, b| a.0.cmp(&b.0));
    let trackers = visible_trackers(g, scope, &tasks);
    Ok(TreePage {
        tasks,
        groups: group_list.into_iter().map(|(_, g)| g).collect(),
        orgs: visible_orgs(g, scope),
        trackers,
        total,
        next_cursor,
        generated_at: g.now,
    })
}

/// `work { action: tree, filters?, cursor?, limit?, per_task? }`.
pub fn tree(store: &Mutex<Store>, scope: &OrgScope, args: &TreeArgs) -> Result<TreePage, IpcError> {
    check_filters(&args.filters)?;
    let g = {
        let s = lock(store)?;
        Graph::load(&s)?
    };
    tree_of(&g, scope, args)
}

/// Parse a task id: `item:<n>` or `ref:<KEY>` (normalised).
pub fn parse_task_id(raw: &str) -> Result<String, IpcError> {
    let raw = raw.trim();
    if let Some(n) = raw.strip_prefix("item:") {
        let id: i64 = n
            .parse()
            .map_err(|_| bad(format!("task_id {raw:?} is not item:<id> or ref:<KEY>")))?;
        return Ok(format!("item:{id}"));
    }
    if let Some(k) = raw.strip_prefix("ref:") {
        return Ok(format!("ref:{}", crate::store::normalize_work_ref(k)?));
    }
    Err(bad(format!(
        "task_id {raw:?} is not item:<id> or ref:<KEY>"
    )))
}

fn not_found_task(task_id: &str) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("task {task_id} not found"))
}

/// The task `task_id` names for `scope`, built with every session: a bare
/// key a sync bound to an item answers as that item (with the key in
/// `aliases`). Out of scope answers exactly as unknown.
pub(crate) fn find_task(
    g: &Graph,
    scope: &OrgScope,
    task_id: &str,
    with_rejected: bool,
) -> Result<(WorkTask, Vec<String>), IpcError> {
    let task_id = parse_task_id(task_id)?;
    let built = build_tasks(g, scope);
    let others = active_tasks_by_session(&built);
    let mut aliases = Vec::new();
    let mut target = task_id.clone();
    if !built.iter().any(|b| b.task_id == target) {
        if let Some(k) = task_id.strip_prefix("ref:") {
            if let Some(i) = g
                .items
                .values()
                .filter(|i| {
                    i.item.key.as_deref() == Some(k) || i.item.aliases.iter().any(|a| a == k)
                })
                .min_by_key(|i| (i.item.tracker_id.is_none(), i.item.id))
            {
                target = format!("item:{}", i.item.id);
                aliases.push(task_id.clone());
            }
        }
    }
    let b = built
        .iter()
        .find(|b| b.task_id == target)
        .ok_or_else(|| not_found_task(&task_id))?;
    Ok((
        to_task(g, b, usize::MAX, &others, true, with_rejected),
        aliases,
    ))
}

/// `work { action: task, task_id }`.
pub fn task(store: &Mutex<Store>, scope: &OrgScope, task_id: &str) -> Result<TaskDetail, IpcError> {
    let s = lock(store)?;
    let g = Graph::load(&s)?;
    let (task, aliases) = find_task(&g, scope, task_id, true)?;
    let item = task.item_id.and_then(|i| g.items.get(&i));
    // The tracker that might serve the whole description, and the key to name
    // it by — flattened as every other `fence_ticket` call site flattens it
    // (`defuse` alone does not fold a newline), because it ends up inside
    // fleet's own notice line.
    let tracker = item
        .and_then(|i| i.item.tracker_id)
        .and_then(|t| g.trackers.get(&t));
    let flat_key = item
        .and_then(|i| i.item.key.as_deref())
        .map(|k| k.split_whitespace().collect::<Vec<_>>().join(" "));
    let description = item
        .and_then(|i| i.meta.description.clone())
        .filter(|d| !d.trim().is_empty())
        .map(|d| match scope {
            // An agent reads it: the same audience and marker as `lookup`,
            // the start brief and the card, so the same helper — the notice
            // when what is shown is less than the tracker holds, and the
            // offer of `describe` when its provider serves one. This path's
            // own cap is the Work view's (600), narrower than the 2000-char
            // excerpt a row carries, so the notice fires here for a
            // description the other paths show whole — which is exactly
            // right: it names what THIS answer shows. Before this, the task
            // detail cut at 600 in silence, through the bare
            // `fence_untrusted` the rest of the branch replaced.
            OrgScope::Host { .. } => crate::mcp::guard::fence_ticket(
                &d,
                "a tracker ticket",
                DESCRIPTION_MAX_CHARS,
                item.and_then(|i| i.meta.description_chars),
                crate::service::trackers::tickets::describe_offer(tracker, flat_key.as_deref()),
            ),
            // A person reads it on a phone or the desktop (bound or not): as
            // is, exactly as before.
            _ => d.chars().take(DESCRIPTION_MAX_CHARS).collect(),
        });
    // The newest past session the caller sees, and its conversation's
    // newest summary, note or outcome.
    let last_outcome = task
        .sessions
        .iter()
        .filter(|l| l.state == "ended")
        .max_by_key(|l| (l.ended_at.unwrap_or(0), l.link_id))
        .map(|l| -> Result<LastOutcome, IpcError> {
            let ids: Vec<String> = g
                .links
                .iter()
                .find(|v| v.link.id == l.link_id)
                .and_then(|v| v.link.snap_claude_ids.as_deref())
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default();
            let rows = s.journal_for_conversations(&ids)?;
            let pick = rows.iter().rev().find(|r| {
                matches!(r.kind.as_str(), "summary" | "note" | "outcome") && r.body.is_some()
            });
            let summary = pick.and_then(|r| r.body.clone()).map(|b| match scope {
                OrgScope::Host { .. } => {
                    crate::mcp::guard::fence_untrusted(&b, "a work journal", OUTCOME_MAX_CHARS)
                }
                _ => b.chars().take(OUTCOME_MAX_CHARS).collect(),
            });
            Ok(LastOutcome {
                at: l.ended_at.unwrap_or(0),
                name: l.name.clone(),
                host: l.host.clone(),
                branch: l.branch.clone(),
                pr_url: l.pr_url.clone(),
                end_reason: l.end_reason.clone(),
                summary,
                summary_kind: pick.map(|r| r.kind.clone()),
            })
        })
        .transpose()?;
    let rules = g
        .rules
        .iter()
        .filter(|r| {
            r.enabled && rule_matches(r, item, task.key.as_deref(), &task.title, &task.repos)
        })
        .map(|r| r.id)
        .collect();
    // Who placed it is the unrestricted caller's to read: a device name is
    // not a scoped caller's to learn (an unassigned task is placed by
    // bound clients of several orgs, M14.1c).
    let placement = g.placements.get(&task.task_id).cloned().map(|mut p| {
        if !scope.is_all() {
            p.updated_by = None;
        }
        p
    });
    Ok(TaskDetail {
        task,
        aliases,
        description,
        last_outcome,
        placement,
        rules,
    })
}

fn brief_of(g: &Graph, task_id: &str, item: Option<&ViewItem>, ref_key: Option<&str>) -> TaskBrief {
    let tracker = item
        .and_then(|i| i.item.tracker_id)
        .and_then(|t| g.trackers.get(&t));
    TaskBrief {
        task_id: task_id.to_string(),
        key: item
            .and_then(|i| i.item.key.clone())
            .or_else(|| ref_key.map(str::to_string)),
        title: item.map(|i| i.item.title.clone()).unwrap_or_default(),
        kind: match item {
            Some(i) if i.item.tracker_id.is_some() => "tracker",
            Some(_) => "local",
            None => "ref",
        }
        .into(),
        status_category: item
            .map(|i| i.item.status_category.clone())
            .filter(|c| !c.is_empty()),
        status_name: item.and_then(|i| i.item.status_name.clone()),
        url: item.and_then(|i| i.item.url.clone()),
        unavailable: item.is_some_and(|i| i.item.unavailable_at.is_some()),
        org_id: item.and_then(|i| g.item_org(i)),
        tracker_name: tracker.map(|t| t.name.clone()),
    }
}

/// `work { action: session_tasks, session_id }`: every link of the
/// session the caller sees — active, suggested, rejected and ended — each
/// with its task.
pub fn session_tasks(
    store: &Mutex<Store>,
    scope: &OrgScope,
    session_id: i64,
) -> Result<SessionTasks, IpcError> {
    let s = lock(store)?;
    let g = Graph::load(&s)?;
    let row = g
        .sessions
        .get(&session_id)
        .filter(|r| scope.sees_row(r))
        .ok_or_else(|| crate::service::orgs::not_found("session", session_id))?;
    let mine: Vec<ViewLink> = s.work_view_session_links(session_id)?;
    let built = build_tasks(&g, scope);
    let others = active_tasks_by_session(&built);
    let visible_tasks: HashMap<&str, &Built<'_>> =
        built.iter().map(|b| (b.task_id.as_str(), b)).collect();
    let mut links = Vec::new();
    let mut primary = None;
    for l in &mine {
        let Some(tid) = Graph::task_id_of(l) else {
            continue;
        };
        // The link as the graph holds it (its session join), so its state
        // and visibility are the tree's.
        let Some(gl) = g.links.iter().find(|x| x.link.id == l.link.id) else {
            continue;
        };
        // A live session's link that ended on a branch change reads
        // `ended` here, as history.
        let Some(state) = g.state_of(gl) else {
            continue;
        };
        if !g.link_visible(scope, gl) {
            continue;
        }
        let Some(b) = visible_tasks.get(tid.as_str()) else {
            continue;
        };
        if state == "active" && gl.link.is_primary {
            primary = Some(gl.link.id);
        }
        links.push(SessionTaskLink {
            link: task_link(&g, gl, state, &tid, &others, true),
            task: brief_of(&g, &tid, b.item, b.ref_key.as_deref()),
        });
    }
    links.sort_by(|a, b| {
        state_rank(&a.link.state)
            .cmp(&state_rank(&b.link.state))
            .then((!a.link.primary).cmp(&!b.link.primary))
            .then(b.link.created_at.cmp(&a.link.created_at))
    });
    Ok(SessionTasks {
        session_id,
        org_id: row.org_id,
        primary_link_id: primary,
        links,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ReviewKey(u8, i64, String);

fn review_rank(kind: &str) -> u8 {
    match kind {
        "cross_org" => 0,
        "unavailable" => 1,
        "no_primary" => 2,
        _ => 3,
    }
}

/// [`review`] over a loaded graph.
pub(crate) fn review_of(
    g: &Graph,
    scope: &OrgScope,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<ReviewPage, IpcError> {
    let limit = limit
        .unwrap_or(REVIEW_DEFAULT_LIMIT)
        .clamp(1, TREE_MAX_LIMIT);
    let built = build_tasks(g, scope);
    let task_of: HashMap<i64, &Built<'_>> = built
        .iter()
        .flat_map(|b| b.visible.iter().map(move |(l, _)| (l.link.id, b)))
        .collect();
    // Every live session with a primary, visible or not: "no primary" is
    // judged on the whole session, so an invisible primary never reads as
    // a missing one.
    let has_primary: BTreeSet<i64> = g
        .links
        .iter()
        .filter(|l| l.link.ended_at.is_none() && l.link.is_primary && l.link.state == "confirmed")
        .filter_map(|l| l.session_id)
        .collect();
    let mut items: Vec<(ReviewKey, ReviewItem)> = Vec::new();
    let mut by_session: BTreeMap<i64, Vec<(&ViewLink, &str)>> = BTreeMap::new();
    for b in &built {
        for (l, st) in &b.visible {
            if let Some(sid) = l
                .session_id
                .filter(|_| matches!(*st, "active" | "suggested"))
            {
                by_session.entry(sid).or_default().push((l, st));
            }
        }
    }
    let make = |kind: &str,
                l: &ViewLink,
                row: &SessionRow,
                why: Vec<String>,
                alts: Vec<ReviewAlternative>| {
        let b = task_of.get(&l.link.id).copied();
        let tid = b.map(|b| b.task_id.clone()).unwrap_or_default();
        ReviewItem {
            review_id: if kind == "no_primary" {
                format!("session:{}", row.id)
            } else {
                format!("link:{}", l.link.id)
            },
            kind: kind.to_string(),
            session_id: row.id,
            session_name: row
                .friendly_name
                .clone()
                .unwrap_or_else(|| row.tmux_name.clone()),
            host: row.host_alias.clone(),
            link_id: l.link.id,
            link_version: l.version,
            task: brief_of(
                g,
                &tid,
                b.and_then(|b| b.item),
                b.and_then(|b| b.ref_key.as_deref()),
            ),
            why,
            strength: l.link.strength.clone(),
            rule: l.link.rule.clone(),
            preselected: l.link.preselected,
            alternatives: alts,
            created_at: l.link.decided_at.unwrap_or(l.link.created_at),
        }
    };
    for (sid, links) in &by_session {
        let Some(row) = g.sessions.get(sid) else {
            continue;
        };
        // A per-host token decides only its own host's sessions, so its
        // inbox holds nothing else.
        if scope.host().is_some_and(|h| h != row.host_alias) {
            continue;
        }
        let suggestions: Vec<&ViewLink> = links
            .iter()
            .filter(|(_, st)| *st == "suggested")
            .map(|(l, _)| *l)
            .collect();
        for (l, st) in links {
            let why = || -> Vec<String> {
                l.link
                    .evidence
                    .iter()
                    .filter_map(|e| serde_json::from_value::<Evidence>(e.clone()).ok())
                    .map(|e| {
                        why_line(
                            &l.link.source,
                            &[serde_json::to_value(e).unwrap_or_default()],
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let kind = match *st {
                "suggested" => Some("suggestion"),
                "active" if needs_review(g, l, st) => {
                    let item = l.link.item_id.and_then(|i| g.items.get(&i));
                    if item.is_some_and(|i| i.item.unavailable_at.is_some()) {
                        Some("unavailable")
                    } else {
                        Some("cross_org")
                    }
                }
                _ => None,
            };
            let Some(kind) = kind else {
                continue;
            };
            let alts = if kind == "suggestion" {
                suggestions
                    .iter()
                    .filter(|o| o.link.id != l.link.id)
                    .filter_map(|o| {
                        let b = task_of.get(&o.link.id)?;
                        Some(ReviewAlternative {
                            link_id: o.link.id,
                            task_id: b.task_id.clone(),
                            key: b
                                .item
                                .and_then(|i| i.item.key.clone())
                                .or_else(|| b.ref_key.clone()),
                            title: b.item.map(|i| i.item.title.clone()).unwrap_or_default(),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut w = why();
            if w.is_empty() {
                w.push(match kind {
                    "cross_org" => "the task's org and the session's differ".into(),
                    "unavailable" => "the tracker no longer answers for this ticket".into(),
                    _ => why_line(&l.link.source, &[]),
                });
            }
            let it = make(kind, l, row, w, alts);
            items.push((
                ReviewKey(review_rank(kind), -it.created_at, it.review_id.clone()),
                it,
            ));
        }
        let confirmed: Vec<&ViewLink> = links
            .iter()
            .filter(|(_, st)| *st == "active")
            .map(|(l, _)| *l)
            .collect();
        if !confirmed.is_empty() && !has_primary.contains(sid) {
            let newest = confirmed
                .iter()
                .max_by_key(|l| (l.link.decided_at.unwrap_or(l.link.created_at), l.link.id))
                .copied();
            if let Some(l) = newest {
                let it = make(
                    "no_primary",
                    l,
                    row,
                    vec![format!(
                        "{} tasks and none is primary; the sidebar cannot group it",
                        confirmed.len()
                    )],
                    Vec::new(),
                );
                items.push((
                    ReviewKey(
                        review_rank("no_primary"),
                        -it.created_at,
                        it.review_id.clone(),
                    ),
                    it,
                ));
            }
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let hash = "review".to_string();
    let start = match cursor.filter(|c| !c.trim().is_empty()) {
        None => 0,
        Some(c) => {
            let after: ReviewKey = decode_cursor(c, &hash)?;
            items.partition_point(|(k, _)| *k <= after)
        }
    };
    let total = items.len() as u32;
    let page: Vec<(ReviewKey, ReviewItem)> =
        items.into_iter().skip(start).take(limit + 1).collect();
    let more = page.len() > limit;
    let page: Vec<(ReviewKey, ReviewItem)> = page.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last().map(|(k, _)| encode_cursor(hash.clone(), k))
    } else {
        None
    };
    Ok(ReviewPage {
        items: page.into_iter().map(|(_, i)| i).collect(),
        total,
        next_cursor,
    })
}

/// `work { action: review, cursor?, limit? }`: suggestions and conflicts
/// the caller may decide.
pub fn review(
    store: &Mutex<Store>,
    scope: &OrgScope,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<ReviewPage, IpcError> {
    let g = {
        let s = lock(store)?;
        Graph::load(&s)?
    };
    review_of(&g, scope, cursor, limit)
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
