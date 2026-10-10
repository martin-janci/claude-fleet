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
//!   its session: the live row ([`OrgScope::sees_row_org_only`]), or for a past
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
use super::status::effective_status;
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
/// What a job mirror's title reads as when the reader may not read the
/// dispatch behind it (multi-user M1, T5's review). Deliberately the same
/// words `Store::job_title` falls back to for a prompt with no first line, so
/// a withheld title is indistinguishable from an unremarkable one and the
/// fence leaks nothing by its own shape.
pub const JOB_TITLE_WITHHELD: &str = "Delegated job";

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
    /// false hides archived tasks (done or every link archived, none
    /// active) into archived_hidden; absent shows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// One person the task is assigned to in its tracker, by name (any
    /// case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// One tracker column: the tracker's own status name ("QA Review"),
    /// any case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    /// What a section under each org is: group (default; a person, rule,
    /// tracker container, repo or key), org (one section per org), person,
    /// mission, account or repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// Any of these orgs (ids, or "none" for unassigned): the Work panel's
    /// organisation chips, several at once. Applies with `org` when both
    /// are set. An older hub ignores it and shows every org.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orgs: Vec<IdOrWord>,
    /// Any of these stages ([`STAGE_VALUES`], what [`WorkTask::stage`]
    /// says): the Work panel's status chips, several at once. Applies with
    /// `status` when both are set. An older hub ignores it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
}

/// [`WorkTask::stage`]'s values, in board order.
pub const STAGE_VALUES: [&str; 5] = ["backlog", "in_progress", "in_review", "blocked", "done"];

/// [`WorkTreeFilters::group_by`]'s values.
pub const GROUP_BY_VALUES: [&str; 6] = ["group", "org", "person", "mission", "account", "repo"];

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
    /// The date the work is due, `YYYY-MM-DD` (the Board card's "You · Fri").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
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
    /// No active session, and the task is done or every one of its links
    /// (at least one of them ended) is archived: the tree hides it when
    /// asked to (`filters.archived: false`). Judged over every link, not
    /// only the caller's: a fenced caller must not see a task as archived
    /// while another host or org still works on it.
    #[serde(default)]
    pub archived: bool,
    /// Where the item came from: `manual` (a person wrote it in Fleet),
    /// `agent` (a delegated job's mirror), `proposed` (an agent's accepted
    /// proposal) or `detected` (a tracker ticket, a bare key).
    #[serde(default)]
    pub origin: String,
    /// The project a native task runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// That project as `owner/repo` (or `repo` for a local owner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
    /// A native subtask's parent (`item:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// A job mirror's state (the delegated job's `state`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
    /// The item has no title: `title` is borrowed from its first session's
    /// name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub title_derived: bool,
    /// Agent proposals under this task waiting for a person's decision.
    #[serde(default)]
    pub open_proposals: u32,
    /// The task waits for another work item that is not done (a
    /// `work_item_deps` edge, inside a mission or not), and is not done
    /// itself. Derived on every read, never stored (redesign step 6.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// What it waits for, as task ids (`item:<id>`): only the items this
    /// caller may see, so `blocked` can be true with this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// What its sessions have spent, in micro-USD: the usage of every
    /// distinct session this caller sees on an active or ended link. A
    /// session that worked on two tasks counts in both.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cost_micros: i64,
    /// Where the task stands, one word of [`STAGE_VALUES`]: `done`, then
    /// `blocked` (it waits for another task), `in_review` (its tracker
    /// column says review, or a live session has a pull request),
    /// `in_progress` (its tracker says so, or a session works on it), else
    /// `backlog`. Derived on every read; empty from an older hub.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stage: String,
    /// What a rule, Jev or an LLM proposes about this task, one per
    /// feature (step 2.8): the same shape as `SessionRow::proposals`. Empty,
    /// and absent on the wire, when nothing proposes anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<crate::store::DecisionProposal>,
    /// Its acceptance lines (orchestration O3, the Board's "Finishes when
    /// …", Orbit Fleet G7.6). Absent when it has none, and from an older hub.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    /// The mission it belongs to and its wave there (the Board card's chip,
    /// G7.6): only on a tree read that asked ([`TreeArgs::with_missions`])
    /// or groups by mission, and only for a mission this caller may read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<TaskMission>,
}

/// [`WorkTask::mission`]: a mission's id and name, and the wave of the
/// mission's dependency graph the task sits in (W1 first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMission {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave: Option<u32>,
}

/// A section header while a tree read counts it: the group, how many tasks,
/// the smallest sort key among them and their summed cost.
type GroupAcc = (GroupRef, u32, SortKey, i64);

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// A native subtask on a task page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtaskView {
    pub task_id: String,
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    /// manual | proposed (accepted) | agent
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// Live confirmed links to it (on sessions the caller sees).
    pub live_sessions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
}

/// An agent's proposed subtask. `why` and `notes` are agent text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalView {
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    pub at: i64,
    /// Jev's "may duplicate" (K4, `decide::duplicate`): an open task this
    /// reader sees that the proposal may repeat. Only a live assist answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<DuplicateHint>,
}

/// The existing task a proposal may duplicate, as Jev proposed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateHint {
    pub item_id: i64,
    /// `item:<id>`, the task page to open.
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    /// Always `jev` today.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run, which a person's Merge or Keep both marks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

/// The [`DuplicateHint`] proposal `item_id` carries: its `duplicate`
/// proposal names an item of the graph, open, whose org `scope` sees.
fn duplicate_hint(g: &Graph, scope: &OrgScope, item_id: i64) -> Option<DuplicateHint> {
    let p = g
        .item_proposals
        .get(&item_id)?
        .iter()
        .find(|p| p.feature == crate::service::decide::Feature::Duplicate.as_str())?;
    let target = crate::service::decide::duplicate::item_of(&p.value)?;
    let v = g.items.get(&target)?;
    if target == item_id || v.item.status_category == "done" || !scope.sees_org(g.item_org(v)) {
        return None;
    }
    Some(DuplicateHint {
        item_id: target,
        task_id: format!("item:{target}"),
        key: v.item.key.clone(),
        title: v.item.title.clone(),
        source: p.source.clone(),
        confidence_pct: p.confidence_pct,
        run_id: p.run_id,
    })
}

/// A delegated job under a task. `result` is agent text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    /// The job's mirror item.
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    /// The job's `state`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The worker session's name, while it exists (and the caller sees it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    pub at: i64,
}

/// One conversation's steps, labelled by the session that ran it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepGroup {
    pub label: String,
    pub claude_session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepLine>,
}

/// One step an agent took (its own task tools). `text` is agent text; a
/// `completed` is the agent's word, never a status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepLine {
    pub text: String,
    /// pending | in_progress | completed | cancelled
    pub state: String,
    pub at: i64,
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
    /// The spend of its tasks: the sum of their [`WorkTask::cost_micros`]
    /// over every task the header counts (redesign step 6.3).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cost_micros: i64,
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
    /// Tasks that passed every other filter but were hidden as archived
    /// (over the whole result, not the page).
    #[serde(default)]
    pub archived_hidden: u32,
    /// Tasks the view's filters hide that would show with none set (the
    /// archived switch kept): the "Hidden by filters" row. 0 for a
    /// section's read (`filters.group`).
    #[serde(default)]
    pub hidden_by_filters: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub generated_at: i64,
    /// The sections [`TreeArgs::sections`] asked for, in the order asked,
    /// each paged from the same read (absent when none was asked, and from
    /// a hub built before them: the client then reads each by itself).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<TreeSection>,
    /// `review.total` under the caller's scope, when
    /// [`TreeArgs::with_review_total`] asked (absent from an older hub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_total: Option<u32>,
}

/// One section a tree read pages besides its first page: exactly what a
/// read with `filters.org` = `org_id` (or `"none"`) and `filters.group` =
/// `group_id` answers, cursor included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SectionAsk {
    #[serde(default)]
    pub org_id: Option<i64>,
    pub group_id: String,
    /// 1–200, default 50.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A section's page in [`TreePage::sections`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeSection {
    #[serde(default)]
    pub org_id: Option<i64>,
    pub group_id: String,
    pub tasks: Vec<WorkTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
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
    /// The description's full length as the hub knows it, in characters:
    /// the tracker's own count when the sync recorded one, else the cached
    /// excerpt's. Present whenever `description` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_chars: Option<usize>,
    /// `description` shows less than `description_chars` (the 600-char cap
    /// here, or the cache's own excerpt): a person's screen says so and
    /// points at the ticket. Absent (false) from an older hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub description_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<LastOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// Enabled rules whose conditions match the task (the first one places
    /// it unless a person did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<i64>,
    /// A native task's notes (fenced for an agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The delegated job's result, when this task is a job mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_result: Option<String>,
    /// Native subtasks: a person's, accepted proposals, job mirrors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<SubtaskView>,
    /// Agent proposals waiting for a person's decision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<ProposalView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_proposals: Vec<ProposalView>,
    /// Jobs delegated under this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobView>,
    /// The steps agents took on this task and its subtasks, per
    /// conversation, newest conversation first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepGroup>,
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
    /// 0–100, read off the rule, strength and evidence
    /// (`work::confidence`); `None` from a hub older than step 6.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    pub preselected: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<ReviewAlternative>,
    pub created_at: i64,
    /// Who proposed it, when it is not a rule's reading of a signal: the
    /// decision model's suggestion (J1, rule R12, redesign 6.8). Absent for
    /// every other item, and from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<ReviewProposer>,
    /// The tracker ticket the decision model proposes this suggestion's
    /// LOCAL task duplicates (J7 `tracker_duplicate`, redesign 6.8): a live
    /// assist answer only. Absent otherwise, and from an older hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<ReviewDuplicate>,
}

/// [`ReviewItem::duplicate_of`]: "May duplicate PAY-88 · Proposed by Jev ·
/// 74%".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDuplicate {
    /// The ticket, `item:<id>`.
    pub task_id: String,
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    /// `jev`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
}

/// PURE: the J6 proposer of suggestion `link_id` of a session whose row
/// carries `proposals`: the live `main_ticket` proposal naming it, among
/// `keys` suggestions. `None` for any other link.
pub fn main_ticket_proposer(
    proposals: &[crate::store::DecisionProposal],
    link_id: i64,
    keys: usize,
) -> Option<ReviewProposer> {
    use crate::service::decide::{main_ticket, Feature};
    let p = proposals
        .iter()
        .find(|p| p.feature == Feature::MainTicket.as_str())?;
    (main_ticket::link_of(&p.value) == Some(link_id)).then(|| ReviewProposer {
        source: p.source.clone(),
        reason: main_ticket::reason(keys),
        confidence_pct: p.confidence_pct,
    })
}

/// [`ReviewItem::proposed_by`]: "Proposed by Jev · from the first prompt ·
/// 82%".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewProposer {
    /// `jev`.
    pub source: String,
    /// Why, in fleet's words.
    pub reason: String,
    /// The model's confidence, in whole percent, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
}

/// PURE: who proposed a link that carries `rule` and `evidence`: the
/// decision model for an R12 suggestion, with the confidence its evidence
/// note holds (`82%`); `None` for a rule's own reading.
pub fn proposer_of(rule: Option<&str>, evidence: &[serde_json::Value]) -> Option<ReviewProposer> {
    if rule != Some(crate::service::decide::work_link::RULE) {
        return None;
    }
    let confidence_pct = evidence
        .iter()
        .filter(|e| e.get("signal").and_then(|s| s.as_str()) == Some("jev"))
        .filter_map(|e| e.get("note").and_then(|n| n.as_str()))
        .filter_map(|n| n.trim_end_matches('%').parse::<u8>().ok())
        .next_back();
    Some(ReviewProposer {
        source: "jev".into(),
        reason: "from the first prompt".into(),
        confidence_pct,
    })
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
    /// `health.context_red_pct`: `needs_you` agrees with `list_sessions`.
    pub(crate) context_red_pct: f64,
    /// What the fleet knows beyond the rows (step 2.6): `needs_you` counts a
    /// Blocked session as `list_sessions` does.
    pub(crate) facts: attention::Facts,
    /// Item ids with a live confirmed link whose session is presently
    /// working (native item status §2 rule 3) — one join for the whole
    /// page, looked up per task instead of queried per row. Fenced by the
    /// caller's `OrgScope` (fix round 2): an item whose only working
    /// session belongs to a session this scope cannot see is left out, so
    /// "someone is working on this" never leaks through a task the caller
    /// can otherwise see.
    pub(crate) working_session_items: BTreeSet<i64>,
    /// Session rows that EXIST and this reader may not see (multi-user M1,
    /// T7) — the ids [`Self::load_for`] took out of [`Self::sessions`].
    ///
    /// Kept as a set rather than simply dropped, because "no row" and "a row
    /// you may not see" answer differently: a link whose live session is
    /// merely absent from the graph is judged by its snapshot (`snap_host`,
    /// `snap_org_id`) and would be shown with a name out of the snapshot,
    /// which is the leak this fence exists to close. [`Self::link_visible`]
    /// refuses such a link outright.
    pub(crate) hidden_sessions: BTreeSet<i64>,
    /// `work_links.id` of every link this reader may not see **whose live
    /// participant is gone** (multi-user M1, T9b) — the ENDED half of
    /// [`Self::hidden_sessions`].
    ///
    /// It exists because [`Self::hidden_sessions`] could only ever answer for
    /// a link that still names a row. `ViewLink.session_id` is NULL for every
    /// link of a reaped session (`store/work_view.rs`'s `LEFT JOIN
    /// participants p … AND p.retired_at IS NULL`), so such a link skipped
    /// the fence, fell through to the snapshot arm and was then passed by
    /// `scope.is_all()` — i.e. served, by its `snap_name` / `snap_host` /
    /// `snap_branch`, to every paired client bound to no org. Both sets are
    /// filled by [`crate::service::orgs::link_person_visible_at`], which is
    /// the one statement of the rule, so the Work view and `work { links }`
    /// cannot answer differently for the same link.
    pub(crate) hidden_links: BTreeSet<i64>,
    /// A job mirror's item id → its job's `state`.
    pub(crate) job_states: HashMap<i64, String>,
    /// A task's item id → its proposals waiting for a decision.
    pub(crate) open_proposals: HashMap<i64, u32>,
    /// A task's item id → what a rule, Jev or an LLM proposes about it
    /// (step 2.8, `WorkTask::proposals`).
    pub(crate) item_proposals: HashMap<i64, Vec<crate::store::DecisionProposal>>,
    /// The [`proposer_label`] of every session row that SURVIVED the person
    /// fence, or `None` on the ORG-only load — the text half of
    /// [`Self::hidden_sessions`], for `work_items.proposed_by` (multi-user
    /// M1). See [`Self::proposer_visible`].
    pub(crate) visible_proposers: Option<BTreeSet<String>>,
    /// Every `work_item_deps` edge out of an item in [`Self::items`]: item
    /// id → the items it waits for (redesign step 6.3).
    pub(crate) deps: HashMap<i64, Vec<i64>>,
    /// The scope the graph was loaded for, so a task's `blocked_by` names
    /// only items this caller may see.
    pub(crate) scope: OrgScope,
    /// Item id → the mission it belongs to, `(id, name)`, for the missions
    /// this caller may read: filled by [`tree`] only for a group by mission
    /// (redesign step 6.2), so every other read pays nothing for it.
    pub(crate) missions_by_item: HashMap<i64, (i64, String)>,
    /// Item id → its wave in its mission's dependency graph, for the items
    /// in [`Self::missions_by_item`] ([`fill_mission_waves`]).
    pub(crate) mission_waves: HashMap<i64, u32>,
    /// Account uuid → how it is named (nickname, else email), filled by
    /// [`tree`] only for a group by account.
    pub(crate) account_labels: HashMap<String, String>,
}

/// How a session is named when an agent proposes a subtask in its name:
/// `"<friendly name, else tmux name> · <host alias>"`.
///
/// **One spelling, two readers** (multi-user M1): `work_link { propose }`
/// writes it into `work_items.proposed_by`, and [`Graph::build`] computes it
/// again for the rows the person fence hides, so that
/// [`Graph::proposer_hidden`] can recognise one. The two were the same
/// `format!` in two files, which is how the fence would have drifted into
/// passing everything the first time somebody added a space.
pub(crate) fn proposer_label(row: &SessionRow) -> String {
    let name = row.friendly_name.as_deref().unwrap_or(&row.tmux_name);
    format!("{name}{SEPARATOR}{}", row.host_alias)
}

/// What a [`proposer_label`] puts between the session and its host, and
/// therefore the one mark that says a stored `proposed_by` is a SESSION's
/// label rather than a caller's (`master`, `client:phone`).
const SEPARATOR: &str = " · ";

impl Graph {
    /// The ORG-only load: for the reads and writes that answer a TASK and
    /// never a session row — `service::work::structure`'s rules, saved
    /// views, placements and org moves, whose exemptions are written down in
    /// `mcp::tools::tests`'s `WORK_ACTION_NO_GATE`.
    ///
    /// Every Work-view read that answers session rows takes
    /// [`Self::load_for`] instead.
    pub(crate) fn load(s: &Store, scope: &OrgScope) -> Result<Graph, IpcError> {
        Self::build(s, scope, None)
    }

    /// The load every Work-view READ uses: the caller's WHOLE scope, so a
    /// session this person may not see is not in the graph at all and no
    /// projection built off the graph can carry it (multi-user M1, T7) —
    /// **for both shapes a session reaches a projection in**: a LIVE row
    /// (taken out of [`Graph::sessions`] into [`Graph::hidden_sessions`]) and
    /// an ENDED link whose participant has been reaped, which has no live row
    /// to take out and is listed in [`Graph::hidden_links`] instead (T9b).
    ///
    /// The second half was missing, and the doc promised it anyway: `tree`,
    /// `task`, `review` and `org_impact` all rest on this sentence.
    pub(crate) fn load_for(
        s: &Store,
        view: &crate::service::view_scope::ViewScope,
    ) -> Result<Graph, IpcError> {
        Self::build(s, &view.org, Some(view))
    }

    fn build(
        s: &Store,
        scope: &OrgScope,
        view: Option<&crate::service::view_scope::ViewScope>,
    ) -> Result<Graph, IpcError> {
        let mut sessions: HashMap<i64, SessionRow> = s
            .list_all_sessions()?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        // The PERSON fence, before anything is derived from a row: whose row
        // it is, what was shared with this caller, and §4.4's host clauses —
        // none of which an `OrgScope` can express (it is `All` for the master
        // and for every paired client bound to no org alike).
        let hidden_sessions: BTreeSet<i64> = match view {
            Some(v) => {
                let hidden: BTreeSet<i64> = sessions
                    .values()
                    .filter(|r| !v.sees_session_row(r).is_visible())
                    .map(|r| r.id)
                    .collect();
                sessions.retain(|id, _| !hidden.contains(id));
                hidden
            }
            None => BTreeSet::new(),
        };
        let sessions = sessions;
        let links = s.work_view_links()?;
        // The ENDED half of the same fence, through the one predicate
        // (multi-user M1, T9b). A link whose participant has been reaped has
        // no live row above to take out of the graph, so it is named here;
        // `link_visible` and `impact_of` test both sets before the snapshot
        // arm. The memo makes a page cost one lookup per distinct
        // conversation, and `is_internal` keeps the hub's own readers (and
        // the scale fixture, which loads with `ViewScope::internal`) on the
        // old cost.
        let hidden_links: BTreeSet<i64> = match view {
            Some(v) if !v.is_internal() => {
                let mut seen: std::collections::BTreeMap<String, bool> = Default::default();
                let mut hidden = BTreeSet::new();
                for l in &links {
                    if l.session_id.is_some() {
                        continue;
                    }
                    if !crate::service::orgs::link_person_visible_memo(
                        s, v, &l.link, None, &mut seen,
                    )? {
                        hidden.insert(l.link.id);
                    }
                }
                hidden
            }
            _ => BTreeSet::new(),
        };
        // Fence the live signal by scope: a pair whose session this scope
        // cannot see (`OrgScope::sees_row_org_only`, the same check `link_visible`
        // uses) does not lift its item, even though the raw query found it.
        let working_session_items: BTreeSet<i64> = s
            .work_items_with_working_session()?
            .into_iter()
            .filter(|(_, session_id)| {
                sessions
                    .get(session_id)
                    // `hidden_sessions` above has already emptied every
                    // person-invisible row out of `sessions` when a
                    // `ViewScope` was supplied; this clause is what still
                    // fences the org-only `Graph::load` path, whose readers
                    // answer tasks and never a session row.
                    .is_some_and(|row| scope.sees_row_org_only(row))
            })
            .map(|(item_id, _)| item_id)
            .collect();
        let items: HashMap<i64, ViewItem> = s
            .work_view_items()?
            .into_iter()
            .map(|i| (i.item.id, i))
            .collect();
        // Machine proposals about an item are about the ITEM (its org's
        // work data, fenced with the item itself), and carry only ids and
        // vocabulary words.
        let item_proposals: HashMap<i64, Vec<crate::store::DecisionProposal>> = s
            .current_proposals(crate::store::PROPOSAL_SUBJECT_WORK_ITEM)?
            .into_iter()
            .filter_map(|(id, v)| Some((id.parse::<i64>().ok()?, v)))
            .filter(|(id, _)| items.contains_key(id))
            .collect();
        let mut open_proposals: HashMap<i64, u32> = HashMap::new();
        for i in items.values() {
            if let (Some("proposed"), Some(parent)) =
                (i.item.proposal_state.as_deref(), i.item.parent_id)
            {
                *open_proposals.entry(parent).or_default() += 1;
            }
        }
        // A job mirror's state is the state of a DISPATCH, and both ends of a
        // dispatch are sessions (multi-user M1, the review of main's new
        // `Graph` fields). `work { task }` served it — and `JobView.result`,
        // the worker's own output — to anyone who could see the work item,
        // with no fence of any kind: `job_states_by_item` answered a state
        // string, so the one predicate that judges a task
        // (`tasks::task_visible_in_scope_pure`) could not even be asked. The
        // store now hands over the `tasks` row and the predicate runs here,
        // against the ALREADY person-filtered `sessions` map — so an end this
        // reader cannot see resolves to `None` and the predicate fails closed
        // on it, which is its documented behaviour for an unreadable end.
        //
        // `view: None` is the ORG-only load, whose readers answer tasks and
        // never a session row (`Graph::load`'s doc, and the
        // `WORK_ACTION_NO_GATE` rows it names); it keeps every state, as it
        // keeps every session row.
        // The labels of the rows that SURVIVED the fence, so
        // `proposer_visible` can recognise one (multi-user M1). `None` is the
        // ORG-only load, which applies no person fence anywhere.
        let visible_proposers: Option<BTreeSet<String>> =
            view.map(|_| sessions.values().map(proposer_label).collect());
        let job_states = job_states_from(s, &sessions, view)?;
        let item_ids: Vec<i64> = items.keys().copied().collect();
        let mut deps: HashMap<i64, Vec<i64>> = HashMap::new();
        for e in s.item_deps(&item_ids)? {
            deps.entry(e.item_id).or_default().push(e.depends_on);
        }
        let placements: HashMap<String, Placement> = s
            .work_placements()?
            .into_iter()
            .map(|p| (p.task_id.clone(), p))
            .collect();
        let rules = s.work_rules()?;
        // K5: a `work_placement` proposal names its group by label here.
        let item_proposals = crate::service::decide::work_placement::label_proposals(
            s,
            item_proposals,
            &placements,
            &rules,
        )?;
        Ok(Graph {
            now: crate::service::catalog::now_secs(),
            items,
            // M1 reads `links` above, to build `hidden_links`, so the local
            // is used here rather than a second `work_view_links` query.
            links,
            sessions,
            trackers: s.list_trackers()?.into_iter().map(|t| (t.id, t)).collect(),
            orgs: s.list_orgs()?,
            projects: s.list_projects()?.into_iter().map(|p| (p.id, p)).collect(),
            placements,
            rules,
            context_red_pct: crate::service::health::context_red_pct(s),
            facts: s.attention_facts(),
            working_session_items,
            hidden_sessions,
            hidden_links,
            job_states,
            // NOT fenced, and that is the judgement rather than an omission:
            // this is a count of ITEMS (the `proposed` children of one
            // parent), not of sessions. A proposal's own session metadata is
            // its `proposed_by`, which `hidden_proposers` withholds; what is
            // left — its title and `why` — is text a person wrote into the
            // shared graph for the people who decide it, and the count
            // carries none of it anyway.
            //
            // **The reason stops at proposals, and used to be written as
            // though it covered every item** (multi-user M1, T5's review).
            // It does not cover a JOB MIRROR: `create_agent_task_item` fills
            // an `agent` child's title from the first line of the dispatch
            // PROMPT and its notes from the prompt itself, so a mirror's text
            // is one session's instruction to another — §4.3 content of both
            // ends, not shared work structure. Mirrors are fenced on
            // `g.job_states` wherever their text is served (`JobView`, and
            // `SubtaskView.title` in `native_work`); they are not counted
            // here, since a mirror is never `proposal_state = 'proposed'`.
            open_proposals,
            item_proposals,
            visible_proposers,
            deps,
            scope: scope.clone(),
            missions_by_item: HashMap::new(),
            mission_waves: HashMap::new(),
            account_labels: HashMap::new(),
        })
    }

    /// Is this item a JOB MIRROR whose dispatch the reader may not read — so
    /// its title and notes must be withheld (multi-user M1, T5's review)?
    ///
    /// A mirror's `title` is the first line of the dispatch PROMPT and its
    /// `notes` are the prompt itself (`Store::create_agent_task_item`), so a
    /// mirror's text is one session's instruction to another: §4.3 content of
    /// both ends of the dispatch, not shared work structure. The reasoning
    /// that let it through — "its title and `why` are item data in the shared
    /// graph, as every other item's are" — is true of a PROPOSAL and false of
    /// a mirror; the `open_proposals` note in [`Graph::build`] records that.
    ///
    /// The fence is [`Graph::job_states`], the map already filtered by
    /// `tasks::task_visible_in_scope_pure` (which fails closed on a dispatch
    /// end this reader cannot resolve). Asking it here rather than
    /// re-deriving the predicate is what keeps every surface of one mirror —
    /// its own page, its row in the tree, its row among a parent's subtasks,
    /// and `JobView` — from disagreeing about the same job.
    ///
    /// `job_states` is unfiltered for the ORG-only load (`Graph::load`, whose
    /// readers answer tasks and never a session row), so this is `false`
    /// there, exactly as every other person fence is.
    /// Takes the ROW, not a [`ViewItem`], so the one predicate serves both
    /// readers: the graph's own items and the children `native_children`
    /// reads straight from the store (which are not all in
    /// [`Graph::items`]).
    pub(crate) fn mirror_text_hidden(&self, item: &crate::store::WorkItemRow) -> bool {
        item.origin.as_deref() == Some("agent") && !self.job_states.contains_key(&item.id)
    }

    /// Is this link one the reader may not see — its LIVE row fenced, or (for
    /// a link whose participant is gone) its recorded conversations fenced?
    ///
    /// **The one clause every projection built off a link must ask first**
    /// (multi-user M1, T9b). It must run before the snapshot arms, which
    /// would otherwise name the session by `snap_name` and its machine by
    /// `snap_host` — exactly what the fence withholds.
    pub(crate) fn link_hidden(&self, l: &ViewLink) -> bool {
        l.session_id
            .is_some_and(|id| self.hidden_sessions.contains(&id))
            || self.hidden_links.contains(&l.link.id)
    }

    /// May this reader be told that `proposed_by` proposed a subtask?
    ///
    /// `work_items.proposed_by` is the [`proposer_label`] of the session an
    /// agent proposed in the name of — its name and its machine, stored as
    /// TEXT when the proposal was written, with no session id beside it. It
    /// is session metadata by §4.3's definition, exactly as a link's
    /// `snap_name` / `snap_host` are, and it was served to every reader of
    /// the parent item.
    ///
    /// The rule is **allow-list, not deny-list**, and that is the whole of
    /// why this is written the way it is:
    ///
    /// * a label that matches a row which survived the fence is served — the
    ///   reader can see that session anyway;
    /// * a session-shaped label that matches nothing is WITHHELD. A deny-list
    ///   against the hidden rows would have passed exactly the case that
    ///   needs it most: a reaped session leaves no row to hide, so its name
    ///   would have become readable by everybody the moment it died — which
    ///   is the same hole `Graph::hidden_links` exists to close for an ended
    ///   link (T9b). The cost is that an attribution fades when its session
    ///   is reaped, for its owner too;
    /// * a label that is not session-shaped passes: `master`,
    ///   `client:phone`, and every caller label `work_link { propose }`
    ///   records when it names no session, carry no [`SEPARATOR`] and are
    ///   not sessions at all.
    ///
    /// `None` is the ORG-only load ([`Graph::load`]), which applies no person
    /// fence anywhere and must not start here.
    pub(crate) fn proposer_visible(&self, proposed_by: &str) -> bool {
        match &self.visible_proposers {
            None => true,
            Some(ok) => !proposed_by.contains(SEPARATOR) || ok.contains(proposed_by),
        }
    }

    /// An item's own org: its tracker's, or a local item's (M14), or — for a
    /// native subtask, which `insert_native` leaves with no `org_id` — its
    /// parent's. `Store::item_org` in memory (a test holds the two equal).
    ///
    /// One level only, matching the SQL: `parent_for_new_child` refuses a
    /// native parent that is itself a subtask, so there is no deeper chain to
    /// walk and no cycle to guard against.
    ///
    /// **It asks no person fence, and that was checked rather than assumed**
    /// (multi-user M1, the review of main's new `Graph` fields). Everything
    /// it reads is item data — `work_items.tracker_id`, a tracker's `org_id`,
    /// a local item's own org, and the PARENT item's same two fields: no
    /// link, no session, nothing [`Self::link_hidden`] could be asked about.
    /// The parent fallback cannot widen the answer either, because a native
    /// subtask's real org IS its parent's (`insert_native` leaves the column
    /// NULL and the SQL `Store::item_org` reads it the same way) — so this
    /// reports the org such an item already belongs to rather than lending it
    /// one.
    pub(crate) fn item_org(&self, item: &ViewItem) -> Option<i64> {
        self.own_item_org(item).or_else(|| {
            item.item
                .parent_id
                .and_then(|p| self.items.get(&p))
                .and_then(|p| self.own_item_org(p))
        })
    }

    /// The org an item carries itself, with no parent fallback.
    fn own_item_org(&self, item: &ViewItem) -> Option<i64> {
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
    pub(crate) fn session_org(&self, l: &ViewLink) -> Option<i64> {
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
    pub(crate) fn state_of(&self, l: &ViewLink) -> Option<&'static str> {
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
        // The person fence first, and before the `is_all()` shortcut: a link
        // whose session this reader may not see is refused whatever its org
        // says, and must NOT fall through to the snapshot arm below, which
        // would judge it by `snap_host` and show it by its snapshot name
        // (multi-user M1, T7; the ENDED half is T9b's `hidden_links`).
        if self.link_hidden(l) {
            return false;
        }
        // This is the org boundary, not a privacy fence: and it is deliberately placed AFTER
        // `link_hidden`, so the person fence runs first and this shortcut can never skip it.
        if scope.is_all() {
            return true;
        }
        if !scope.sees_org(self.link_org(l)) {
            return false;
        }
        // Each arm below is the ORG half only; `link_hidden` at the top of
        // this function is the person half, and it runs before the
        // `is_all()` shortcut precisely so it cannot be skipped.
        match self.row_of(l) {
            Some(row) => match scope {
                // M2's fence: a host's past work is its own host's.
                OrgScope::Host { alias, .. } if l.link.ended_at.is_some() => {
                    // Org half; `link_hidden` is the person half.
                    row.host_alias == *alias && scope.sees_row_org_only(row)
                }
                // Org half; `link_hidden` is the person half.
                _ => scope.sees_row_org_only(row),
            },
            None => {
                let host = l.link.snap_host.as_deref().unwrap_or_default();
                match scope {
                    OrgScope::Host { alias, .. } => host == alias,
                    // The snapshot arm's org half; `link_hidden`'s
                    // `hidden_links` half covers exactly this shape.
                    _ => scope.sees_session_org_only(host, l.link.org_id),
                }
            }
        }
    }

    fn is_mine(&self, item: &ViewItem) -> bool {
        let Some(t) = item.item.tracker_id.and_then(|t| self.trackers.get(&t)) else {
            return false;
        };
        t.config.account_id.is_some() && t.config.account_id == item.assignee_id
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
    /// Every link with a wire state, visible or not: `archived` is the
    /// task's, whoever reads it (never listed, only judged).
    all: Vec<(&'g ViewLink, &'static str)>,
    /// A per-host token's fence: work of it on its own host.
    on_own_host: bool,
}

fn build_tasks<'g>(g: &'g Graph, scope: &OrgScope) -> Vec<Built<'g>> {
    let mut by_task: BTreeMap<String, Built<'g>> = BTreeMap::new();
    for item in g.items.values() {
        // A proposal waiting for a decision (or rejected) is not a task:
        // only its parent's page lists it. An accepted one is a subtask.
        if matches!(
            item.item.proposal_state.as_deref(),
            Some("proposed" | "rejected")
        ) {
            continue;
        }
        let id = format!("item:{}", item.item.id);
        by_task.insert(
            id.clone(),
            Built {
                task_id: id,
                item: Some(item),
                ref_key: None,
                visible: Vec::new(),
                any_links: 0,
                all: Vec::new(),
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
                    all: Vec::new(),
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
        b.all.push((l, state));
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
    // This is the org boundary, not a privacy fence: it decides whether the TASK appears, and a
    // task's key and title are work data — the same answer `local.rs::person_visible_links`
    // writes down for a local item. Every link and session UNDER the task is person-fenced
    // (`b.visible`, built from `link_visible`).
    if scope.is_all() {
        return true;
    }
    let fence = b.item.and_then(|i| g.item_org(i));
    if fence.is_some() && !scope.sees_org(fence) {
        return false;
    }
    let shown = b.visible.iter().any(|(_, st)| *st != "rejected");
    match scope {
        // This is the org boundary, not a privacy fence: the same answer as
        // this function's first guard, in its other spelling — whether the
        // TASK appears. A task's key and title are work data and survive the
        // person fence exactly as a local item's do, while every link and
        // session under it is person-fenced by `Graph::build`.
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
        return why_of(&e);
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

/// One piece of evidence, in one line.
fn why_of(e: &Evidence) -> String {
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
        Some("jev") => "Jev proposed",
        _ => "seen",
    };
    let text: String = e.text.chars().take(60).collect();
    // The decision model's confidence rides the note (`82%`).
    let note = match (e.signal, e.note.as_deref()) {
        (super::resolve::Signal::Jev, Some(n)) => format!(" ({n})"),
        _ => String::new(),
    };
    format!("{what} {text}{note} · {}", e.rule)
        .trim()
        .to_string()
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
        };
    }
    if let Some(repo) = repos.first() {
        return GroupRef {
            id: format!("repo:{repo}"),
            label: repo.clone(),
            source: "repo".into(),
            rule_id: None,
            tracker_value: None,
        };
    }
    if let Some(prefix) = key.and_then(key_prefix) {
        return GroupRef {
            id: format!("key:{prefix}"),
            label: prefix,
            source: "key".into(),
            rule_id: None,
            tracker_value: None,
        };
    }
    GroupRef {
        id: "none".into(),
        label: "No group".into(),
        source: "none".into(),
        rule_id: None,
        tracker_value: None,
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

/// What a link's session is called: the live row's name, else the
/// snapshot's (`row` is the live session the caller judges the link by).
pub(crate) fn link_name(row: Option<&SessionRow>, l: &ViewLink) -> String {
    match row {
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
    }
}

/// A task's kind: `tracker`, `local` or `ref` (a bare key, no item).
pub(crate) fn task_kind(item: Option<&ViewItem>) -> &'static str {
    match item {
        Some(i) if i.item.tracker_id.is_some() => "tracker",
        Some(_) => "local",
        None => "ref",
    }
}

/// Does this live session need a person (`list_sessions`' judgement)?
fn needs_you_of(g: &Graph, row: Option<&SessionRow>) -> bool {
    row.is_some_and(|r| attention::needs_attention_in(r, g.context_red_pct, &g.facts).is_some())
}

/// One session a task counts: the live session, else the participant it
/// was (one per session), else the link itself. A session with two past
/// links to one task is one past session.
fn session_key(l: &ViewLink) -> (u8, i64) {
    match (l.session_id, l.link.participant_id) {
        (Some(sid), _) => (0, sid),
        (None, Some(pid)) => (1, pid),
        (None, None) => (2, l.link.id),
    }
}

/// `needs_you` is [`needs_you_of`] the link's live session, computed once
/// by the caller.
fn task_link(
    g: &Graph,
    l: &ViewLink,
    state: &str,
    task_id: &str,
    others: &HashMap<i64, BTreeSet<String>>,
    with_evidence: bool,
    needs_you: bool,
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
        name: link_name(row, l),
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
        needs_you,
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

/// A task with everything but its `sessions`: what the filters, the order,
/// the section headers and `archived_hidden` read. Its links are kept in
/// list order, each with whether its session needs a person, so the
/// sessions are built ([`TaskSummary::into_task`]) only for the tasks a
/// page returns.
struct TaskSummary<'g> {
    /// `sessions` empty, `sessions_more` every listed link.
    task: WorkTask,
    links: Vec<(&'g ViewLink, &'static str, bool)>,
}

impl TaskSummary<'_> {
    /// The task with its first `per_task` sessions.
    fn into_task(
        self,
        g: &Graph,
        per_task: usize,
        others: &HashMap<i64, BTreeSet<String>>,
        with_evidence: bool,
    ) -> WorkTask {
        let TaskSummary { task, links } = self;
        with_sessions(task, &links, g, per_task, others, with_evidence)
    }

    /// [`Self::into_task`] of a summary that answers more than one page
    /// (the first page and a section of the same read).
    fn to_page_task(
        &self,
        g: &Graph,
        per_task: usize,
        others: &HashMap<i64, BTreeSet<String>>,
    ) -> WorkTask {
        with_sessions(self.task.clone(), &self.links, g, per_task, others, false)
    }
}

fn with_sessions(
    mut task: WorkTask,
    links: &[(&ViewLink, &'static str, bool)],
    g: &Graph,
    per_task: usize,
    others: &HashMap<i64, BTreeSet<String>>,
    with_evidence: bool,
) -> WorkTask {
    let sessions: Vec<TaskLink> = links
        .iter()
        .take(per_task)
        .map(|(l, st, needs_you)| {
            task_link(g, l, st, &task.task_id, others, with_evidence, *needs_you)
        })
        .collect();
    task.sessions_more = links.len().saturating_sub(sessions.len()) as u32;
    task.sessions = sessions;
    task
}

/// The job mirrors this reader may read, as item id → job state — the ONE
/// place the dispatch fence is written (multi-user M1, T5's review).
///
/// `sessions` must already be the PERSON-filtered map
/// ([`Graph::hidden_sessions`] has been taken out of it), because that is
/// what makes `task_visible_in_scope_pure` fail closed: an end this reader
/// cannot see resolves to `None`, which is its documented refusal.
///
/// `view: None` is the ORG-only load ([`Graph::load`], whose readers answer
/// tasks and never a session row): every job is kept, as every session row
/// is.
fn job_states_from(
    s: &Store,
    sessions: &HashMap<i64, SessionRow>,
    view: Option<&crate::service::view_scope::ViewScope>,
) -> Result<HashMap<i64, String>, IpcError> {
    Ok(s.job_tasks_by_item()?
        .into_iter()
        .filter(|(_, t)| match view {
            Some(v) => crate::service::tasks::task_visible_in_scope_pure(
                t,
                t.requester_session_id.and_then(|id| sessions.get(&id)),
                t.worker_session_id.and_then(|id| sessions.get(&id)),
                v,
            ),
            None => true,
        })
        .map(|(item_id, t)| (item_id, t.state))
        .collect())
}

/// [`job_states_from`] for a reader that has no [`Graph`] to hand — it reads
/// and filters the session map itself (`work::local::local_items`).
///
/// Separate from the `Graph` path rather than the other way round so a pass
/// that already holds the sessions map does not read `sessions` twice.
pub(crate) fn visible_job_states(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
) -> Result<HashMap<i64, String>, IpcError> {
    let sessions: HashMap<i64, SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| view.sees_session_row(r).is_visible())
        .map(|r| (r.id, r))
        .collect();
    job_states_from(s, &sessions, Some(view))
}

fn summarize<'g>(g: &Graph, b: &Built<'g>, with_rejected: bool) -> TaskSummary<'g> {
    let item = b.item;
    let key = item
        .and_then(|i| i.item.key.clone())
        .or_else(|| b.ref_key.clone());
    // A job mirror's title is the dispatch prompt's first line, withheld from
    // a reader of neither end (`Graph::mirror_text_hidden`). Fenced HERE, the
    // first time the title is read, so neither the group label nor the
    // derived-title fallback below can carry it.
    let title = item
        .map(|i| {
            if g.mirror_text_hidden(&i.item) {
                JOB_TITLE_WITHHELD.to_string()
            } else {
                i.item.title.clone()
            }
        })
        .unwrap_or_default();
    let tracker = item
        .and_then(|i| i.item.tracker_id)
        .and_then(|t| g.trackers.get(&t));
    let mut links: Vec<(&'g ViewLink, &'static str)> = b
        .visible
        .iter()
        .filter(|(_, st)| with_rejected || *st != "rejected")
        .copied()
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
    // Every count is of sessions, not links: a session with two past
    // links to the task is one past session.
    let mut active_sessions = BTreeSet::new();
    let mut ended_sessions = BTreeSet::new();
    let mut suggested_sessions = BTreeSet::new();
    let mut needs_you = false;
    let mut review = false;
    let mut live_pr = false;
    let mut last: Option<i64> = None;
    let mut repos: Vec<String> = Vec::new();
    let mut link_orgs: BTreeSet<Option<i64>> = BTreeSet::new();
    // Session id -> its spend, once per session however many links it has.
    let mut spent: BTreeMap<i64, i64> = BTreeMap::new();
    let mut listed = Vec::with_capacity(links.len());
    for (l, st) in links {
        let row = g.session_of(l);
        if let (Some(r), "active" | "ended") = (row, st) {
            spent.insert(r.id, r.usage.usage_cost_micros);
        }
        let link_needs_you = needs_you_of(g, row);
        listed.push((l, st, link_needs_you));
        match st {
            "active" => {
                active_sessions.insert(session_key(l));
            }
            "ended" => {
                ended_sessions.insert(session_key(l));
            }
            "suggested" => {
                suggested_sessions.insert(session_key(l));
            }
            _ => {}
        }
        if st == "rejected" {
            continue;
        }
        if st == "active" && link_needs_you {
            needs_you = true;
        }
        if st == "active"
            && row
                .and_then(|r| r.pr_url.as_deref())
                .or(l.link.snap_pr_url.as_deref())
                .is_some_and(|u| !u.trim().is_empty())
        {
            live_pr = true;
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
        if matches!(st, "active" | "ended") {
            link_orgs.insert(g.session_org(l));
        }
    }
    let counts = TaskCounts {
        active: active_sessions.len() as u32,
        ended: ended_sessions.len() as u32,
        suggested: suggested_sessions.len() as u32,
    };
    // `status_category` here is `item_status`'s live-lifted value (fix
    // round 3 checked this deliberately, not assumed safe): a `done` that
    // the live rule lifts to `in_progress` (a session resumes an
    // untracked/legacy `done`) never flips `archived` to true either way,
    // because the same confirmed link that justifies the lift is itself
    // `active` in `b.all` below — see
    // `a_legacy_done_item_a_session_resumes_is_lifted_and_stays_unarchived`.
    let status_category = item.and_then(|i| item_status(g, i));
    // Over every link: `counts.active` is the caller's, and a session on a
    // host or in an org it cannot see still keeps the task in work.
    let archived = !b.all.iter().any(|(_, st)| *st == "active")
        && (status_category.as_deref() == Some("done") || all_links_archived(&b.all));
    let (blocked, blocked_by) = match item {
        Some(i) if status_category.as_deref() != Some("done") => blocked_on(g, i.item.id),
        _ => (false, Vec::new()),
    };
    let cost_micros = spent.values().sum();
    let status_name = item.and_then(|i| i.item.status_name.clone());
    let stage = stage_of(
        status_category.as_deref(),
        status_name.as_deref(),
        blocked,
        live_pr,
        counts.active,
    );
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
    // The group reads the item's own title, never a borrowed one.
    let group = group_of(g, &b.task_id, item, key.as_deref(), &title, &repos);
    let origin = origin_of(item.map(|i| &i.item));
    let project_id = item.and_then(|i| i.item.project_id);
    let project_label = project_id.and_then(|p| project_label(g, p));
    let parent_task_id = item
        .filter(|i| is_native(&i.item))
        .and_then(|i| i.item.parent_id)
        .map(|p| format!("item:{p}"));
    let job_state = item.and_then(|i| g.job_states.get(&i.item.id).cloned());
    let open_proposals = item
        .and_then(|i| g.open_proposals.get(&i.item.id).copied())
        .unwrap_or(0);
    let proposals = item
        .and_then(|i| g.item_proposals.get(&i.item.id).cloned())
        .unwrap_or_default();
    let (title, title_derived) = match listed
        .iter()
        .find(|(_, st, _)| *st != "rejected")
        .or(listed.first())
    {
        Some((l, _, _)) if title.trim().is_empty() => (link_name(g.row_of(l), l), true),
        _ => (title, false),
    };
    let task = WorkTask {
        task_id: b.task_id.clone(),
        item_id: item.map(|i| i.item.id),
        key,
        title,
        url: item.and_then(|i| i.item.url.clone()),
        kind: task_kind(item).into(),
        tracker_id: item.and_then(|i| i.item.tracker_id),
        tracker_name: tracker.map(|t| t.name.clone()),
        provider: tracker.map(|t| t.provider.clone()),
        tracker_state: tracker.map(|t| t.state.clone()),
        status_category,
        status_name,
        resolution: item.and_then(|i| i.item.resolution.clone()),
        unavailable: item.is_some_and(|i| i.item.unavailable_at.is_some()),
        unavailable_reason: item.and_then(|i| i.item.unavailable_reason.clone()),
        assignees: item.map(|i| i.item.assignees.clone()).unwrap_or_default(),
        due_at: item.and_then(|i| i.item.due_at.clone()),
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
        sessions_more: listed.len() as u32,
        sessions: Vec::new(),
        archived,
        origin,
        project_id,
        project_label,
        parent_task_id,
        job_state,
        title_derived,
        open_proposals,
        blocked,
        blocked_by,
        cost_micros,
        stage: stage.into(),
        proposals,
        done_when: item.map(|i| i.item.done_when.clone()).unwrap_or_default(),
        mission: item.and_then(|i| {
            let (id, name) = g.missions_by_item.get(&i.item.id)?;
            Some(TaskMission {
                id: *id,
                name: name.clone(),
                wave: g.mission_waves.get(&i.item.id).copied(),
            })
        }),
    };
    TaskSummary {
        task,
        links: listed,
    }
}

/// [`WorkTask::stage`]: the first of done, blocked, in review, in progress
/// that holds, else backlog.
fn stage_of(
    status_category: Option<&str>,
    status_name: Option<&str>,
    blocked: bool,
    live_pr: bool,
    active: u32,
) -> &'static str {
    if status_category == Some("done") {
        "done"
    } else if blocked {
        "blocked"
    } else if live_pr || status_name.is_some_and(|n| n.to_lowercase().contains("review")) {
        "in_review"
    } else if status_category == Some("in_progress") || active > 0 {
        "in_progress"
    } else {
        "backlog"
    }
}

/// Whether item `id` waits for work that is not done, and which of those
/// items the caller may see (as `item:<id>`). An item missing from the graph
/// still blocks; so does one the caller may not see, which is then left out
/// of the list (it would otherwise name work outside the caller's orgs).
fn blocked_on(g: &Graph, id: i64) -> (bool, Vec<String>) {
    let Some(deps) = g.deps.get(&id) else {
        return (false, Vec::new());
    };
    let mut blocked = false;
    let mut named = Vec::new();
    for dep in deps {
        let item = g.items.get(dep);
        if item.and_then(|i| item_status(g, i)).as_deref() == Some("done") {
            continue;
        }
        blocked = true;
        let visible = item.is_some_and(|i| {
            // This is the org boundary, not a privacy fence: whether a scoped
            // caller is told the id of a work item in another org. Items are
            // the org's work data; nothing about a person is named here.
            g.scope.is_all() || g.item_org(i).is_some_and(|o| g.scope.sees_org(Some(o)))
        });
        if visible {
            named.push(format!("item:{dep}"));
        }
    }
    (blocked, named)
}

/// An item's origin; a row an older hub wrote (and a bare key) reads as
/// `detected`.
fn origin_of(item: Option<&crate::store::WorkItemRow>) -> String {
    item.and_then(|i| i.origin.clone())
        .unwrap_or_else(|| "detected".into())
}

/// A native item: a person's, an agent's proposal, or a job mirror.
fn is_native(item: &crate::store::WorkItemRow) -> bool {
    matches!(
        item.origin.as_deref(),
        Some("manual" | "proposed" | "agent")
    )
}

/// A project as `owner/repo`, or `repo` for a local (or empty) owner.
fn project_label(g: &Graph, project_id: i64) -> Option<String> {
    let p = g.projects.get(&project_id)?;
    Some(if p.owner.is_empty() || p.owner == "local" {
        p.repo.clone()
    } else {
        format!("{}/{}", p.owner, p.repo)
    })
}

fn to_task(
    g: &Graph,
    b: &Built<'_>,
    per_task: usize,
    others: &HashMap<i64, BTreeSet<String>>,
    with_evidence: bool,
    with_rejected: bool,
) -> WorkTask {
    summarize(g, b, with_rejected).into_task(g, per_task, others, with_evidence)
}

/// At least one ended link, and every link but a rejected one archived
/// (the UI-only archive of work graph M7).
fn all_links_archived(links: &[(&ViewLink, &str)]) -> bool {
    let mut ended = false;
    for (l, st) in links {
        match *st {
            "rejected" => continue,
            "ended" => ended = true,
            _ => {}
        }
        if l.archived_at.is_none() {
            return false;
        }
    }
    ended
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
    for o in &f.orgs {
        if let IdOrWord::Word(w) = o {
            if w != "none" {
                return Err(bad(format!(
                    "filters.orgs holds org ids or \"none\", not {w:?}"
                )));
            }
        }
    }
    if f.orgs.len() > 100 {
        return Err(bad("filters.orgs names more than 100 orgs"));
    }
    for s in &f.stages {
        if !STAGE_VALUES.contains(&s.as_str()) {
            return Err(bad(format!(
                "filters.stages holds backlog, in_progress, in_review, blocked or done, not {s:?}"
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
    if f.assignee
        .as_deref()
        .is_some_and(|q| q.chars().count() > 200)
    {
        return Err(bad("filters.assignee is longer than 200 characters"));
    }
    if f.status_name
        .as_deref()
        .is_some_and(|q| q.chars().count() > 200)
    {
        return Err(bad("filters.status_name is longer than 200 characters"));
    }
    if let Some(by) = f.group_by.as_deref() {
        if !GROUP_BY_VALUES.contains(&by) {
            return Err(bad(format!(
                "filters.group_by is group, org, person, mission, account or repo, not {by:?}"
            )));
        }
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
    if !f.orgs.is_empty()
        && !f.orgs.iter().any(|o| match o {
            IdOrWord::Id(id) => t.org_id == Some(*id),
            IdOrWord::Word(_) => t.org_id.is_none(),
        })
    {
        return false;
    }
    if !f.stages.is_empty() && !f.stages.contains(&t.stage) {
        return false;
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
    if let Some(who) = f
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        if !t
            .assignees
            .iter()
            .any(|a| a.trim().eq_ignore_ascii_case(who))
        {
            return false;
        }
    }
    if let Some(col) = f
        .status_name
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        if !t
            .status_name
            .as_deref()
            .is_some_and(|n| n.trim().eq_ignore_ascii_case(col))
        {
            return false;
        }
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
    !hidden_as_archived(t, f)
}

/// An archived task stays out of a tree listing only when the caller asks
/// (`archived: false`): a client from before the archive (a fleet-mobile
/// that never sends it and has no "N hidden" row) keeps seeing every task.
/// An explicit Done filter, or "past only" (past work is archived work),
/// shows them anyway (a Done stage chip too). Only the tree hides: a direct read (`task`,
/// `session_tasks`, `review`) answers archived tasks as any other.
fn hidden_as_archived(t: &WorkTask, f: &WorkTreeFilters) -> bool {
    t.archived
        && f.archived == Some(false)
        && f.status.as_deref() != Some("done")
        && !f.stages.iter().any(|s| s == "done")
        && f.has.as_deref() != Some("past_only")
}

/// The section a task sits in under [`WorkTreeFilters::group_by`]
/// (redesign step 6.2), under its org as always: one section per org
/// (`org`), its first assignee (`person`), its mission (`mission`), the
/// account its sessions run on (`account`, an active one first) or its repo
/// (`repo`). `None` keeps its own group (`group`, the default). A task with
/// nothing to group by sits in `none`, last.
fn regroup(g: &Graph, s: &TaskSummary<'_>, by: &str) -> Option<GroupRef> {
    let t = &s.task;
    let mk = |id: String, label: String, source: &str| GroupRef {
        id,
        label,
        source: source.into(),
        rule_id: None,
        tracker_value: None,
    };
    let none = |label: &str| mk("none".into(), label.into(), "none");
    Some(match by {
        "org" => mk("org".into(), "All tasks".into(), "org"),
        "person" => match t.assignees.iter().map(|a| a.trim()).find(|a| !a.is_empty()) {
            Some(a) => mk(
                format!("person:{}", a.to_lowercase()),
                a.to_string(),
                "person",
            ),
            None => none("No assignee"),
        },
        "mission" => match t.item_id.and_then(|i| g.missions_by_item.get(&i)) {
            Some((id, name)) => mk(format!("mission:{id}"), name.clone(), "mission"),
            None => none("No mission"),
        },
        "account" => {
            let account_of = |want_active: bool| {
                s.links.iter().find_map(|(l, st, _)| {
                    if want_active && *st != "active" {
                        return None;
                    }
                    let row = g.sessions.get(&l.session_id?)?;
                    row.account_uuid.clone().filter(|u| !u.is_empty())
                })
            };
            match account_of(true).or_else(|| account_of(false)) {
                Some(uuid) => {
                    let label = g.account_labels.get(&uuid).cloned().unwrap_or_else(|| {
                        format!("Account {}", uuid.chars().take(8).collect::<String>())
                    });
                    mk(format!("account:{uuid}"), label, "account")
                }
                None => none("No account"),
            }
        }
        "repo" => match t.repos.first().or(t.project_label.as_ref()) {
            Some(r) => mk(format!("repo:{r}"), r.clone(), "repo"),
            None => none("No repo"),
        },
        _ => return None,
    })
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
    /// Sections to page from the same read (a client's open sections), so
    /// one refresh is one graph load. At most [`TREE_MAX_SECTIONS`].
    pub sections: Vec<SectionAsk>,
    /// Add `review_total`, from the same read.
    pub with_review_total: bool,
    /// Name each task's mission and wave ([`WorkTask::mission`], the
    /// Board, G7.6). Off, a read pays nothing for it.
    pub with_missions: bool,
}

/// The most sections one tree read pages.
pub const TREE_MAX_SECTIONS: usize = 100;

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
            // This is the org boundary, not a privacy fence: which COMPANIES'
            // trackers a brief names. A `TrackerBrief` is a tracker's id,
            // name and kind — org configuration, no session.
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
        .map(|b| summarize(g, b, false).into_task(g, per_task, &others, false))
        .collect()
}

/// [`tree`] over a graph already loaded (tests and the scale budget).
pub(crate) fn tree_of(g: &Graph, scope: &OrgScope, args: &TreeArgs) -> Result<TreePage, IpcError> {
    check_filters(&args.filters)?;
    if args.sections.len() > TREE_MAX_SECTIONS {
        return Err(bad(format!(
            "sections asks for {} sections; at most {TREE_MAX_SECTIONS}",
            args.sections.len()
        )));
    }
    let limit = args
        .limit
        .unwrap_or(TREE_DEFAULT_LIMIT)
        .clamp(1, TREE_MAX_LIMIT);
    let per_task = args.per_task.unwrap_or(PER_TASK_DEFAULT).min(PER_TASK_MAX);
    let org_names: HashMap<i64, String> = g.orgs.iter().map(|o| (o.id, o.name.clone())).collect();
    // Every task summarised; its sessions are built only if the page
    // returns it.
    let built = build_tasks(g, scope);
    let others = active_tasks_by_session(&built);
    // Every filter but the archived one: what it hides is counted, so the
    // view can say how many archived tasks are out of sight.
    let with_archived = WorkTreeFilters {
        archived: Some(true),
        ..args.filters.clone()
    };
    let mut archived_hidden = 0u32;
    // The view with no filter but the archived switch (and the grouping):
    // a task it shows that this read's filters hide is "hidden by filters".
    let unfiltered = WorkTreeFilters {
        archived: args.filters.archived,
        group_by: args.filters.group_by.clone(),
        ..WorkTreeFilters::default()
    };
    let count_hidden = args.filters.group.is_none() && unfiltered != args.filters;
    let mut hidden_by_filters = 0u32;
    // Section headers count every task under the filters but the group
    // one, so every section of the view has its header and count.
    let mut groups: BTreeMap<(Option<i64>, String), GroupAcc> = BTreeMap::new();
    let mut summaries: Vec<TaskSummary<'_>> =
        built.iter().map(|b| summarize(g, b, false)).collect();
    // Before any filter: `filters.group` names a section of this grouping.
    if let Some(by) = args.filters.group_by.as_deref() {
        for summary in &mut summaries {
            if let Some(group) = regroup(g, summary, by) {
                summary.task.group = group;
            }
        }
    }
    let mut matching: Vec<(SortKey, usize)> = Vec::new();
    for (i, summary) in summaries.iter().enumerate() {
        let t = &summary.task;
        if !matches_filters(t, &with_archived, false) {
            if count_hidden && matches_filters(t, &unfiltered, false) {
                hidden_by_filters += 1;
            }
            continue;
        }
        if hidden_as_archived(t, &args.filters) {
            if args
                .filters
                .group
                .as_deref()
                .is_none_or(|gid| gid == t.group.id)
            {
                archived_hidden += 1;
            }
            continue;
        }
        let key = sort_key(t, &org_names);
        let e = groups
            .entry((t.org_id, t.group.id.clone()))
            .or_insert_with(|| (t.group.clone(), 0, key.clone(), 0));
        e.1 += 1;
        e.3 += t.cost_micros;
        if key < e.2 {
            e.2 = key.clone();
        }
        if args
            .filters
            .group
            .as_deref()
            .is_none_or(|gid| gid == t.group.id)
        {
            matching.push((key, i));
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
    let page: Vec<(SortKey, usize)> = matching.into_iter().skip(start).take(limit + 1).collect();
    let more = page.len() > limit;
    let page: Vec<(SortKey, usize)> = page.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last().map(|(k, _)| encode_cursor(hash.clone(), k))
    } else {
        None
    };
    let tasks: Vec<WorkTask> = page
        .into_iter()
        .map(|(_, i)| summaries[i].to_page_task(g, per_task, &others))
        .collect();
    let sections = if args.sections.is_empty() {
        Vec::new()
    } else {
        section_pages(g, &summaries, &others, &org_names, args, per_task)
    };
    let review_total = if args.with_review_total {
        Some(review_of(g, scope, None, Some(1))?.total)
    } else {
        None
    };
    let mut group_list: Vec<(SortKey, TreeGroup)> = groups
        .into_iter()
        .map(|((org, _), (group, count, key, cost_micros))| {
            (
                SortKey(key.0, key.1, key.2, key.3, 0, 0, String::new()),
                TreeGroup {
                    org_id: org,
                    org_name: org.and_then(|o| org_names.get(&o).cloned()),
                    group,
                    count,
                    cost_micros,
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
        archived_hidden,
        hidden_by_filters,
        next_cursor,
        generated_at: g.now,
        sections,
        review_total,
    })
}

/// The filters one section's own read is made with: the view's, narrowed
/// to its org and group (what a client's "Load more" sends, so the cursor
/// here is valid there).
fn section_filters(base: &WorkTreeFilters, org_id: Option<i64>, group_id: &str) -> WorkTreeFilters {
    WorkTreeFilters {
        org: Some(org_id.map_or_else(|| IdOrWord::Word("none".into()), IdOrWord::Id)),
        group: Some(group_id.to_string()),
        ..base.clone()
    }
}

/// A section's bucket: its org and its group id.
type SectionKey<'a> = (Option<i64>, &'a str);

/// [`TreeArgs::sections`], each paged from the summaries already built:
/// exactly the first page a read with [`section_filters`] answers.
fn section_pages(
    g: &Graph,
    summaries: &[TaskSummary<'_>],
    others: &HashMap<i64, BTreeSet<String>>,
    org_names: &HashMap<i64, String>,
    args: &TreeArgs,
    per_task: usize,
) -> Vec<TreeSection> {
    // A section's filters differ from the view's only in the org and the
    // group, which bucket the tasks: every other filter is checked once.
    let rest = WorkTreeFilters {
        org: None,
        group: None,
        ..args.filters.clone()
    };
    let mut buckets: HashMap<SectionKey<'_>, Vec<(SortKey, usize)>> = HashMap::new();
    for (i, s) in summaries.iter().enumerate() {
        let t = &s.task;
        if matches_filters(t, &rest, false) {
            buckets
                .entry((t.org_id, t.group.id.as_str()))
                .or_default()
                .push((sort_key(t, org_names), i));
        }
    }
    for bucket in buckets.values_mut() {
        bucket.sort_by(|a, b| a.0.cmp(&b.0));
    }
    args.sections
        .iter()
        .map(|ask| {
            let limit = ask
                .limit
                .unwrap_or(TREE_DEFAULT_LIMIT)
                .clamp(1, TREE_MAX_LIMIT);
            let hash = filters_hash(&section_filters(&args.filters, ask.org_id, &ask.group_id));
            let all = buckets
                .get(&(ask.org_id, ask.group_id.as_str()))
                .map_or(&[][..], Vec::as_slice);
            let page = &all[..all.len().min(limit)];
            let next_cursor = if all.len() > limit {
                page.last().map(|(k, _)| encode_cursor(hash, k))
            } else {
                None
            };
            TreeSection {
                org_id: ask.org_id,
                group_id: ask.group_id.clone(),
                tasks: page
                    .iter()
                    .map(|(_, i)| summaries[*i].to_page_task(g, per_task, others))
                    .collect(),
                next_cursor,
            }
        })
        .collect()
}

/// [`Graph::mission_waves`]: each mission member's wave, the topological
/// depth over the `work_item_deps` edges between members of the same
/// mission, W1 first; the same depth [`super::graph::build`] gives a
/// mission's page. An edge out of the mission does not count; the store
/// refuses a cycle, and the depth bound is a second net.
fn fill_mission_waves(g: &mut Graph) {
    fn depth(id: i64, g: &Graph, mission: i64, memo: &mut HashMap<i64, u32>, guard: usize) -> u32 {
        if let Some(w) = memo.get(&id) {
            return *w;
        }
        let w = if guard == 0 {
            1
        } else {
            1 + g
                .deps
                .get(&id)
                .map(|ds| {
                    ds.iter()
                        .filter(|d| g.missions_by_item.get(d).map(|m| m.0) == Some(mission))
                        .map(|d| depth(*d, g, mission, memo, guard - 1))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        };
        memo.insert(id, w);
        w
    }
    let mut memo = HashMap::new();
    let guard = g.missions_by_item.len() + 1;
    let ids: Vec<(i64, i64)> = g.missions_by_item.iter().map(|(i, m)| (*i, m.0)).collect();
    for (item, mission) in ids {
        depth(item, g, mission, &mut memo, guard);
    }
    g.mission_waves = memo;
}

/// `work { action: tree, filters?, cursor?, limit?, per_task? }`.
///
/// Takes the caller's whole [`ViewScope`] (multi-user M1, T7): the tree names
/// every session of every task, which spec §4.3 calls content.
pub fn tree(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    args: &TreeArgs,
) -> Result<TreePage, IpcError> {
    let scope = &view.org;
    // A group by mission names only the missions this caller may read
    // (`missions::sees_mission`); read before the graph, which takes the
    // lock itself.
    let by_mission = args.with_missions || args.filters.group_by.as_deref() == Some("mission");
    let readable_missions: HashMap<i64, String> = if by_mission {
        super::missions::missions(store, view)?
            .into_iter()
            .map(|m| (m.id, m.name))
            .collect()
    } else {
        HashMap::new()
    };
    let g = {
        let s = lock(store)?;
        let mut g = Graph::load_for(&s, view)?;
        if by_mission {
            for (item, mission) in s.mission_membership()? {
                if let Some(name) = readable_missions.get(&mission) {
                    g.missions_by_item.insert(item, (mission, name.clone()));
                }
            }
            fill_mission_waves(&mut g);
        }
        if args.filters.group_by.as_deref() == Some("account") {
            for a in s.list_accounts()? {
                let label = a
                    .nickname
                    .clone()
                    .or(a.email.clone())
                    .or(a.display_name.clone());
                if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
                    g.account_labels.insert(a.uuid, label);
                }
            }
        }
        g
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
///
/// The whole [`ViewScope`] (multi-user M1, T7): one task's detail lists every
/// session of that task.
pub fn task(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    task_id: &str,
) -> Result<TaskDetail, IpcError> {
    let scope = &view.org;
    let g = {
        let s = lock(store)?;
        Graph::load_for(&s, view)?
    };
    let (task, aliases) = find_task(&g, scope, task_id, true)?;
    let item = task.item_id.and_then(|i| g.items.get(&i));
    // The newest past session the caller sees, and the conversations it
    // ran (for its last outcome).
    let last = task
        .sessions
        .iter()
        .filter(|l| l.state == "ended")
        .max_by_key(|l| (l.ended_at.unwrap_or(0), l.link_id));
    let conversations: Vec<String> = last
        .and_then(|l| g.links.iter().find(|v| v.link.id == l.link_id))
        .and_then(|v| v.link.snap_claude_ids.as_deref())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    // The one item's description (the graph reads no description), and
    // that journal; its native children, their jobs, live sessions and
    // the steps of every conversation of the task and its subtasks: a
    // second, short lock.
    let (meta, journal, work) = {
        let s = lock(store)?;
        let meta = item
            .map(|i| s.work_item_meta(i.item.id))
            .transpose()?
            .unwrap_or_default();
        // Inside that link's window (P-7): a session switched away kept its
        // conversation, and what it did after the switch is the next task's.
        let journal = match last {
            Some(l) => s.journal_for_conversations_within(
                &conversations,
                s.link_journal_window(l.link_id)?,
            )?,
            None => Vec::new(),
        };
        let work = native_work(&s, &g, scope, item, task.key.as_deref())?;
        (meta, journal, work)
    };
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
    let excerpt = meta.description.clone().filter(|d| !d.trim().is_empty());
    // The full length and whether the answer shows less of it, for every
    // caller: a person's screen has no fence notice to read it from. The
    // larger of the tracker's count and the excerpt's own, so a count that
    // trimmed differently from the excerpt never hides a cut.
    let description_chars = excerpt.as_deref().map(|d| {
        let cached = d.chars().count();
        meta.description_chars
            .and_then(|n| usize::try_from(n).ok())
            .map_or(cached, |n| n.max(cached))
    });
    let shown = excerpt
        .as_deref()
        .map_or(0, |d| d.chars().count().min(DESCRIPTION_MAX_CHARS));
    let description_truncated = description_chars.is_some_and(|n| n > shown);
    let description = excerpt.map(|d| match scope {
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
            meta.description_chars,
            crate::service::trackers::tickets::describe_offer(tracker, flat_key.as_deref()),
        ),
        // A person reads it on a phone or the desktop (bound or not): as
        // is, exactly as before.
        _ => d.chars().take(DESCRIPTION_MAX_CHARS).collect(),
    });
    // The newest past session's conversations' newest summary, note or
    // outcome.
    let last_outcome = last.map(|l| {
        let pick = journal.iter().rev().find(|r| {
            matches!(r.kind.as_str(), "summary" | "note" | "outcome") && r.body.is_some()
        });
        let summary = pick.and_then(|r| r.body.clone()).map(|b| match scope {
            OrgScope::Host { .. } => {
                crate::mcp::guard::fence_untrusted(&b, "a work journal", OUTCOME_MAX_CHARS)
            }
            _ => b.chars().take(OUTCOME_MAX_CHARS).collect(),
        });
        LastOutcome {
            at: l.ended_at.unwrap_or(0),
            name: l.name.clone(),
            host: l.host.clone(),
            branch: l.branch.clone(),
            pr_url: l.pr_url.clone(),
            end_reason: l.end_reason.clone(),
            summary,
            summary_kind: pick.map(|r| r.kind.clone()),
        }
    });
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
        // This is the org boundary, not a privacy fence: `Placement.updated_by` is a device
        // label on shared work structure, and an unassigned task is placed by bound clients of
        // several orgs — which is the reason it is withheld.
        //
        // **Open, and recorded as an owner decision** (T9c found it, T9d put
        // it in the table: `scope_guard_tests::OPEN_QUESTIONS`, so the
        // classification itself carries the question instead of only the
        // prose beside it). The narrower question is whether a PERSON's own
        // device should learn another person's DEVICE NAME: for such a
        // caller `is_all()` is true and the label is withheld from nobody.
        // The eight rules do not cover device identity.
        if !scope.is_all() {
            p.updated_by = None;
        }
        p
    });
    // Agent and third-party text an agent reads is fenced, exactly as the
    // description is; a person reads it as is.
    let fence = |x: String, what: &str| match scope {
        OrgScope::Host { .. } => {
            crate::mcp::guard::fence_untrusted(&x, what, DESCRIPTION_MAX_CHARS)
        }
        _ => x,
    };
    // A job mirror's notes ARE the dispatch prompt, so they go the same way
    // as its title and its result (`Graph::mirror_text_hidden`).
    let notes = item
        .filter(|i| !g.mirror_text_hidden(&i.item))
        .and_then(|i| i.item.notes.clone())
        .filter(|n| !n.trim().is_empty())
        .map(|n| fence(n, "a task's notes"));
    let job_result = work.own_result.map(|r| fence(r, "a job's result"));
    let proposal = |mut p: ProposalView| {
        p.why = p.why.map(|w| fence(w, "an agent's proposal"));
        p.notes = p.notes.map(|n| fence(n, "an agent's proposal"));
        p
    };
    let proposals = work.proposals.into_iter().map(proposal).collect();
    let rejected_proposals = work.rejected_proposals.into_iter().map(proposal).collect();
    let jobs = work
        .jobs
        .into_iter()
        .map(|mut j| {
            j.result = j.result.map(|r| fence(r, "a job's result"));
            j
        })
        .collect();
    let steps = work
        .steps
        .into_iter()
        .map(|mut grp| {
            for st in &mut grp.steps {
                st.text = fence(std::mem::take(&mut st.text), "an agent's step");
            }
            grp
        })
        .collect();
    Ok(TaskDetail {
        task,
        aliases,
        description,
        description_chars,
        description_truncated,
        last_outcome,
        placement,
        rules,
        notes,
        job_result,
        subtasks: work.subtasks,
        proposals,
        rejected_proposals,
        jobs,
        steps,
    })
}

/// A task page's native work, read under the caller's lock: unfenced.
#[derive(Default)]
struct NativeWork {
    own_result: Option<String>,
    subtasks: Vec<SubtaskView>,
    proposals: Vec<ProposalView>,
    rejected_proposals: Vec<ProposalView>,
    jobs: Vec<JobView>,
    steps: Vec<StepGroup>,
}

/// The children of `item` (a person's subtasks, accepted and open and
/// rejected proposals, job mirrors), the jobs, and the steps of every
/// conversation of the task and its subtasks. A child whose own org the
/// caller cannot see is left out; a scoped caller reads only the steps of
/// conversations it sees a link through, and a worker it sees.
///
/// Every session this reads comes out of `g.sessions`, which `Graph::load_for`
/// has already emptied of every person-invisible row into `hidden_sessions`
/// (multi-user M1, T7) — so the two `scope.sees_row_org_only` calls below are
/// the ORG half alone, and `hidden_sessions` is the person half that finishes
/// them (their rows in `ORG_HALF_SITES` say the same).
fn native_work(
    s: &Store,
    g: &Graph,
    scope: &OrgScope,
    item: Option<&ViewItem>,
    key: Option<&str>,
) -> Result<NativeWork, IpcError> {
    let mut out = NativeWork::default();
    let job_of = |task_id: Option<i64>| -> Result<Option<crate::store::TaskRow>, IpcError> {
        task_id
            .map(|t| s.get_task(t))
            .transpose()
            .map(Option::flatten)
    };
    if let Some(i) = item {
        // The SAME fence as `JobView.result` sixty lines below, and for the
        // same reason (multi-user M1, T5's review found this half still
        // open): `own_result` is the result of the dispatch THIS item mirrors
        // — the worker session's own output, the most private thing a
        // dispatch produces — and the reader of the task page is not
        // necessarily a reader of either end of that dispatch.
        //
        // `g.job_states` is the fenced map (`Graph::build`): a job whose
        // `tasks` row this reader may not see is not in it, and
        // `task_visible_in_scope_pure` has already failed closed on an end it
        // cannot resolve. Asking the already-applied fence, rather than
        // re-deriving one here, is what keeps this answer and the `JobView`
        // block from disagreeing about the same job.
        if !g.mirror_text_hidden(&i.item) {
            out.own_result = job_of(i.item.task_id)?.and_then(|t| t.result);
        }
    }
    let children: Vec<crate::store::WorkItemRow> = match item {
        Some(i) => s
            .native_children(i.item.id)?
            .into_iter()
            .filter(|c| {
                let own = g.items.get(&c.id).and_then(|v| g.item_org(v));
                own.is_none() || scope.sees_org(own)
            })
            .collect(),
        None => Vec::new(),
    };
    // `native_work` is only reached from `task`, which builds the graph with
    // `Graph::load_for`: `hidden_sessions` has already emptied every
    // person-invisible row out of `g.sessions`, so this is the ORG half
    // alone (multi-user M1, T10 — the two rows in `ORG_HALF_SITES`).
    let sees_session = |sid: i64| {
        g.sessions
            .get(&sid)
            .is_some_and(|r| scope.sees_row_org_only(r))
    };
    let mut item_ids: BTreeSet<i64> = item.map(|i| i.item.id).into_iter().collect();
    let mut keys: Vec<String> = key.map(str::to_string).into_iter().collect();
    for c in children {
        let proposal = |c: &crate::store::WorkItemRow| ProposalView {
            item_id: c.id,
            key: c.key.clone(),
            title: c.title.clone(),
            why: c.proposal_why.clone(),
            notes: c.notes.clone(),
            // The attribution is session metadata — a name and a machine —
            // so it goes only to a reader who may see that session
            // (multi-user M1; `Graph::proposer_hidden`). The proposal itself
            // stays: its title and `why` are item data.
            proposed_by: c.proposed_by.clone().filter(|by| g.proposer_visible(by)),
            at: c.created_at,
            duplicate: (c.proposal_state.as_deref() == Some("proposed"))
                .then(|| duplicate_hint(g, scope, c.id))
                .flatten(),
        };
        match c.proposal_state.as_deref() {
            Some("proposed") => {
                out.proposals.push(proposal(&c));
                continue;
            }
            Some("rejected") => {
                out.rejected_proposals.push(proposal(&c));
                continue;
            }
            _ => {}
        }
        let live_sessions = s
            .local_item_links(Some(c.id))?
            .iter()
            .filter(|l| l.session_id.is_some_and(sees_session))
            .count() as u32;
        let status = g
            .items
            .get(&c.id)
            .and_then(|v| item_status(g, v))
            .or_else(|| Some(c.status_category.clone()));
        // `g.job_states` is the FENCED map (`Graph::build`): a job whose
        // `tasks` row this reader may not see is not in it, and this is the
        // gate on the whole `JobView` — its `result` is the worker session's
        // own output, the single most private thing a dispatch produces.
        // Asking the already-applied fence rather than re-deriving it is what
        // keeps the two answers (`SubtaskView.job_state` below and this
        // block) from disagreeing about the same job.
        // Is this child a job mirror whose dispatch the reader may not read?
        // Its TITLE is the first line of the dispatch prompt and its notes
        // are the prompt (`Store::create_agent_task_item`), so a mirror's
        // text is one session's instruction to another — not shared work
        // structure, whatever the shape of the row carrying it (multi-user
        // M1, T5's review; the `open_proposals` note in `Graph::build` says
        // where that reasoning went wrong). Same fence as `JobView` below,
        // asked once here so the title and the view cannot disagree.
        let mirror_fenced = g.mirror_text_hidden(&c);
        if c.origin.as_deref() == Some("agent") && g.job_states.contains_key(&c.id) {
            if let Some(job) = job_of(c.task_id)? {
                out.jobs.push(JobView {
                    item_id: c.id,
                    key: c.key.clone(),
                    title: c.title.clone(),
                    state: job.state.clone(),
                    worker: job
                        .worker_session_id
                        .and_then(|w| g.sessions.get(&w))
                        .filter(|r| scope.sees_row_org_only(r))
                        .map(|r| {
                            r.friendly_name
                                .clone()
                                .unwrap_or_else(|| r.tmux_name.clone())
                        }),
                    at: job.finished_at.or(job.started_at).unwrap_or(job.created_at),
                    result: job.result,
                });
            }
        }
        item_ids.insert(c.id);
        if let Some(k) = &c.key {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        out.subtasks.push(SubtaskView {
            task_id: format!("item:{}", c.id),
            item_id: c.id,
            key: c.key.clone(),
            origin: origin_of(Some(&c)),
            status,
            project_id: c.project_id,
            live_sessions,
            job_state: g.job_states.get(&c.id).cloned(),
            // The row stays — it is structure, and the tree that shows the
            // same children would otherwise disagree about how many there
            // are — but its text does not. `JOB_TITLE_WITHHELD` is the label
            // `job_title` itself falls back to for a prompt with no first
            // line, so the answer stays well-formed and says nothing.
            title: if mirror_fenced {
                JOB_TITLE_WITHHELD.to_string()
            } else {
                c.title
            },
        });
    }
    // Every conversation of the task and its subtasks, deduplicated.
    let mut convs: Vec<String> = Vec::new();
    for k in &keys {
        let ids = match s.work_conversation_ids(k) {
            Ok(ids) => ids,
            // A key the journal cannot name has no conversations.
            Err(e) if e.code == codes::E_INVALID => continue,
            Err(e) => return Err(e),
        };
        for id in ids {
            if !convs.contains(&id) {
                convs.push(id);
            }
        }
    }
    let labels = conversation_labels(g, scope, &item_ids, &keys);
    let mut groups: Vec<(i64, StepGroup)> = Vec::new();
    for st in s.current_steps(&convs)? {
        let label = labels.get(&st.claude_session_id);
        // This is the org boundary, not a privacy fence: whether a scoped
        // caller is told the PROJECT a subtask sits in. This function's
        // session half is person-fenced by `hidden_sessions`, above.
        if !scope.is_all() && !label.is_some_and(|(_, visible)| *visible) {
            continue;
        }
        let line = StepLine {
            text: st.text,
            state: st.state,
            at: st.at,
        };
        match groups
            .iter_mut()
            .find(|(_, grp)| grp.claude_session_id == st.claude_session_id)
        {
            Some((newest, grp)) => {
                *newest = (*newest).max(line.at);
                grp.steps.push(line);
            }
            None => groups.push((
                line.at,
                StepGroup {
                    label: label.map_or_else(|| "earlier conversation".into(), |(l, _)| l.clone()),
                    claude_session_id: st.claude_session_id,
                    steps: vec![line],
                },
            )),
        }
    }
    // Newest conversation first (a stable sort keeps first-seen order on a
    // tie).
    groups.sort_by_key(|g| std::cmp::Reverse(g.0));
    out.steps = groups.into_iter().map(|(_, grp)| grp).collect();
    Ok(out)
}

/// Each conversation of these items' (or keys') confirmed links → the name
/// of the session that ran it, and whether the caller sees that link: an
/// ended link's snapshot conversations, a live link's session's current
/// one.
fn conversation_labels(
    g: &Graph,
    scope: &OrgScope,
    item_ids: &BTreeSet<i64>,
    keys: &[String],
) -> HashMap<String, (String, bool)> {
    let mut out: HashMap<String, (String, bool)> = HashMap::new();
    for l in &g.links {
        let ours = l.link.item_id.is_some_and(|i| item_ids.contains(&i))
            || (l.link.item_id.is_none()
                && l.link.ref_key.as_ref().is_some_and(|k| keys.contains(k)));
        if !ours || !matches!(g.state_of(l), Some("active" | "ended")) {
            continue;
        }
        let convs: Vec<String> = if l.link.ended_at.is_some() {
            l.link
                .snap_claude_ids
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default()
        } else {
            g.row_of(l)
                .and_then(|r| r.claude_session_id.clone())
                .into_iter()
                .collect()
        };
        let visible = g.link_visible(scope, l);
        let name = link_name(g.session_of(l), l);
        for c in convs {
            match out.get(&c) {
                Some((_, true)) => {}
                Some((_, false)) if !visible => {}
                _ => {
                    out.insert(c, (name.clone(), visible));
                }
            }
        }
    }
    out
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
        kind: task_kind(item).into(),
        // The same live precedence `to_task` projects (§2, fix round 1):
        // two views of the same item must not disagree about its status.
        // `working_session_items` is the same page-wide one-join set —
        // this adds no second query.
        status_category: item.and_then(|i| item_status(g, i)),
        status_name: item.and_then(|i| i.item.status_name.clone()),
        url: item.and_then(|i| i.item.url.clone()),
        unavailable: item.is_some_and(|i| i.item.unavailable_at.is_some()),
        org_id: item.and_then(|i| g.item_org(i)),
        tracker_name: tracker.map(|t| t.name.clone()),
    }
}

/// The item's status a reader should see (§2), shared by `to_task` and
/// `brief_of` so the tree and a session's own task list can never disagree.
/// `effective_status` itself answers `None` for an empty stored value (fix
/// round 3 moved that guard inside the function, so there is one place to
/// get it right instead of once per caller).
fn item_status(g: &Graph, i: &ViewItem) -> Option<String> {
    effective_status(
        &i.item.status_category,
        i.item.status_set_by.as_deref(),
        &i.item.source,
        g.working_session_items.contains(&i.item.id),
    )
    .map(str::to_string)
}

/// `work { action: session_tasks, session_id }`: every link of the
/// session the caller sees — active, suggested, rejected and ended — each
/// with its task.
pub fn session_tasks(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    session_id: i64,
) -> Result<SessionTasks, IpcError> {
    let scope = &view.org;
    let g = {
        let s = lock(store)?;
        Graph::load_for(&s, view)?
    };
    // `Graph::load_for` empties the person-invisible rows out of
    // `g.sessions`. The org half still has to be asked here, and T10 proved
    // it by deleting the call: `ViewScope::sees_session_facts` returns at its
    // first clause for the hub's own reader, before the org boundary, so an
    // internal-and-narrowed scope is fenced by this and nothing else.
    let row = g
        .sessions
        .get(&session_id)
        // Org half; the person half is `Graph::load_for`'s `hidden_sessions`,
        // which has already emptied this map of rows this caller may not see.
        .filter(|r| scope.sees_row_org_only(r))
        .ok_or_else(|| crate::service::orgs::not_found("session", session_id))?;
    let built = build_tasks(&g, scope);
    let others = active_tasks_by_session(&built);
    let visible_tasks: HashMap<&str, &Built<'_>> =
        built.iter().map(|b| (b.task_id.as_str(), b)).collect();
    let mut links = Vec::new();
    let mut primary = None;
    // The links of the session's live participant (the graph's session
    // join), so their state and visibility are the tree's.
    for gl in g.links.iter().filter(|l| l.session_id == Some(session_id)) {
        let Some(tid) = Graph::task_id_of(gl) else {
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
            link: task_link(
                &g,
                gl,
                state,
                &tid,
                &others,
                true,
                needs_you_of(&g, g.session_of(gl)),
            ),
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

/// The J7 ticket a local item `item_id` may duplicate: its live
/// `tracker_duplicate` proposal, when it names a ticket the graph holds.
fn tracker_duplicate_of(g: &Graph, item_id: Option<i64>) -> Option<ReviewDuplicate> {
    use crate::service::decide::{tracker_duplicate, Feature};
    let id = item_id?;
    let p = g
        .item_proposals
        .get(&id)?
        .iter()
        .find(|p| p.feature == Feature::TrackerDuplicate.as_str())?;
    let ticket = tracker_duplicate::item_of(&p.value)?;
    let t = g.items.get(&ticket)?;
    Some(ReviewDuplicate {
        task_id: format!("item:{ticket}"),
        item_id: ticket,
        key: t.item.key.clone(),
        title: t.item.title.clone(),
        source: p.source.clone(),
        confidence_pct: p.confidence_pct,
    })
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
            confidence: Some(super::confidence::confidence(
                &l.link.source,
                l.link.strength.as_deref(),
                l.link.rule.as_deref(),
                &l.link.evidence,
            )),
            preselected: l.link.preselected,
            alternatives: alts,
            created_at: l.link.decided_at.unwrap_or(l.link.created_at),
            proposed_by: (kind == "suggestion")
                .then(|| proposer_of(l.link.rule.as_deref(), &l.link.evidence))
                .flatten(),
            duplicate_of: (kind == "suggestion")
                .then(|| tracker_duplicate_of(g, l.link.item_id))
                .flatten(),
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
            // One line per piece of evidence that reads as evidence.
            let why = || -> Vec<String> {
                l.link
                    .evidence
                    .iter()
                    .filter_map(|e| serde_json::from_value::<Evidence>(e.clone()).ok())
                    .map(|e| why_of(&e))
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
            let mut it = make(kind, l, row, w, alts);
            // J6 (redesign 6.8): the main ticket among several suggestions.
            if kind == "suggestion" && it.proposed_by.is_none() && suggestions.len() > 1 {
                it.proposed_by = main_ticket_proposer(&row.proposals, l.link.id, suggestions.len());
            }
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
    view: &crate::service::view_scope::ViewScope,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<ReviewPage, IpcError> {
    let scope = &view.org;
    let g = {
        let s = lock(store)?;
        Graph::load_for(&s, view)?
    };
    review_of(&g, scope, cursor, limit)
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
