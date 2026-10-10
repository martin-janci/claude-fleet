//! Work links (roadmap M1b.2): read and decide which work a session is doing.
//! Storage and its rules are in `store::work`; this is the one transport-
//! agnostic entry the MCP tools `work` / `work_link` and the desktop commands
//! share, so a paired desktop and a local one answer the same way.

pub mod abandon;
pub mod agent_handover;
pub mod brief_draft;
pub mod buckets;
pub mod card;
pub mod confidence;
pub mod describe;
pub mod detect;
pub mod graph;
pub mod handover;
pub mod harvest;
pub mod local;
pub mod missions;
pub mod nudge;
pub mod orchestrate;
pub mod plan_import;
pub mod recognize;
pub mod report;
pub mod resolve;
pub mod resume;
pub mod retention;
pub mod run;
#[cfg(test)]
mod scale_tests;
pub mod status;
pub mod steps;
pub mod structure;
pub mod summary;
pub mod tidy;
pub mod today;
pub mod usage;
pub mod verify;
pub mod view;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{Decider, SessionRow, Store, WorkLinkRow, WorkTarget, PERSON_SOURCES};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkParams")]
pub struct WorkArgs {
    /// The session's live links.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Or: ended (past) links to this key.
    #[serde(default)]
    pub key: Option<String>,
    /// Default links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "work_action_schema")]
    pub action: Option<String>,
    /// Ended link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<i64>,
    /// Target host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Add the brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_brief: Option<bool>,
    /// Purge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// Purge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_aliases: Option<Vec<String>>,
    /// Tickets: one tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// mine|sprint|recent|filter:<id>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Tickets: text filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Tickets: max rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Tickets: own tasks too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_local: Option<bool>,
    /// Lookup: a ticket URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Today: unix start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// item:<id> or ref:<KEY>; reject: item:<id> merges into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Tree filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "object_schema")]
    pub filters: Option<view::WorkTreeFilters>,
    /// Next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Sessions per task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_task: Option<usize>,
    /// Tree: sections to page from the same read (a Work view's open
    /// sections, so one refresh is one read). A client's knob, kept out of
    /// the served schema: an assistant pages one section by `filters`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub sections: Option<Vec<view::SectionAsk>>,
    /// Tree: add the review inbox's `total` from the same read. Kept out of
    /// the served schema, as `sections`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub with_review_total: Option<bool>,
    /// Draft rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "object_schema")]
    pub rule: Option<structure::RuleInput>,
    /// Org id; 0 none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// sprint|release
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_id: Option<i64>,
    /// Mission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<i64>,
    /// mission: older events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_event: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkLinkParams")]
pub struct WorkLinkArgs {
    /// Fleet session id.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The decision.
    #[schemars(schema_with = "work_link_action_schema")]
    pub action: String,
    /// Work key, e.g. ABC-123, or a free-form name.
    #[serde(default)]
    pub key: Option<String>,
    /// Or: a work item id.
    #[serde(default)]
    pub item_id: Option<i64>,
    /// For unlink.
    #[serde(default)]
    pub link_id: Option<i64>,
    /// manual (default) | agent | agent_inferred (a suggestion)
    #[serde(default)]
    pub source: Option<String>,
    /// last|brief|fresh
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Target host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Edited brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// Start: a ticket URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Start: the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// Start: several repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<i64>>,
    /// Start: brief Claude with the ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_brief: Option<bool>,
    /// preview_start: have a model draft the brief (one call on the host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_brief: Option<bool>,
    /// Start: session name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Start: worktree name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// trust_project: on/off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Link across orgs anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_cross_org: Option<bool>,
    /// Start: beside a live one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
    /// run: implement|review|test|research|integrate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// link/switch: false refuses when live elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_live: Option<bool>,
    /// Snooze (7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// tidy_apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<tidy::TidyApplyItem>>,
    /// Approved nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
    /// name/create/propose: title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// create/propose: parent, item:<id>.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// create/propose/edit: notes (edit: "" clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Display names ([] clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
    /// Due date, YYYY-MM-DD ("" clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
    /// propose: the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// set_status: todo | in_progress | done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// item:<id> or ref:<KEY>; reject: item:<id> merges into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Version seen (0 none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// Primary link seen; 0 none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_primary: Option<i64>,
    /// false: secondary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
    /// decide_batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "array_schema")]
    pub decisions: Option<Vec<structure::LinkDecision>>,
    /// Group label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Placement note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// From org_impact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact_token: Option<String>,
    /// Org id; 0 none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// rule_save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "object_schema")]
    pub rule: Option<structure::RuleInput>,
    /// rule_delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// view_save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "object_schema")]
    pub view: Option<structure::ViewInput>,
    /// view_delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_id: Option<i64>,
    /// Bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_id: Option<i64>,
    /// Mission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<i64>,
    /// mission_save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "object_schema")]
    pub mission: Option<missions::MissionInput>,
    /// dep: what `item_id` waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<i64>,
    /// accept_many, undo_accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_ids: Option<Vec<i64>>,
    /// propose_tree: [{title, notes?, why?, depends_on?: [{entry: n} |
    /// {item: id}]}].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "array_schema")]
    pub tree: Option<Vec<crate::store::TreeEntry>>,
    /// mission_import: [{step, title, lane?, needs?, status?}]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "array_schema")]
    pub plan: Option<Vec<plan_import::PlanRow>>,
    /// done_when: the item's condition lines (`[]` clears them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<Vec<String>>,
    /// verify: the condition line checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// verify: whether it is met. card_decide: apply (true) or dismiss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// mission_start: one step's key (`run:12`); omitted, the whole wave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// card_decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<i64>,
    /// mission_grant: the autonomy signed, 1 to 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    /// mission_grant: how long it lasts (default 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hours: Option<u32>,
    /// mission_grant: what its workers may spend, in cents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_cents: Option<i64>,
    /// mission_grant: the hosts its runs may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    /// mission_grant: runs at once, at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,
    /// mission_grant: the login its runs bill, a credential profile on the
    /// run's host; omitted, the host's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// today_brief: draft a new brief (true) or answer the last one;
    /// mission_triage: also draft the card's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<bool>,
    /// today_brief: the start of the viewer's day, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

/// `work_link { action: dismiss, item_id }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dismissed {
    #[serde(default)]
    pub dismissed: bool,
}

/// `work_link { action: dismiss, item_id }`: clear an item's "reopened".
pub fn dismiss_reopened(args: &WorkLinkArgs, store: &Mutex<Store>) -> Result<Dismissed, IpcError> {
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "dismiss needs item_id"))?;
    Ok(Dismissed {
        dismissed: lock(store)?.dismiss_reopened(item)?,
    })
}

/// `work_link { action: trust_project }`: the projects whose branch keys
/// now link automatically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTrust {
    #[serde(default)]
    pub trusted: Vec<i64>,
}

/// `work_link { action: trust_project, project_id, on }`.
pub fn trust_project(args: &WorkLinkArgs, store: &Mutex<Store>) -> Result<ProjectTrust, IpcError> {
    let pid = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "trust_project needs project_id"))?;
    let on = args
        .on
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "trust_project needs on"))?;
    let s = lock(store)?;
    Ok(ProjectTrust {
        trusted: detect::set_project_trust(&s, pid, on)?
            .into_iter()
            .collect(),
    })
}

/// `work { action: context }`: the full handover context of a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkContext {
    pub key: String,
    pub text: String,
}

/// `work { action: purge_impact }`: the keys a purge would leave without
/// resumable conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeImpact {
    #[serde(default)]
    pub keys: Vec<String>,
}

/// The `work` read actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkAction {
    Links,
    Context,
    ResumePlan,
    PurgeImpact,
    /// Tracker tickets from the cache (work graph M3.4).
    Tickets,
    /// One ticket by key or URL: the cache, else one live fetch.
    Lookup,
    /// The trackers (no secrets).
    Trackers,
    /// The scope selector's entries: named orgs, uncovered owners, the rest
    /// (work graph M5).
    Scopes,
    /// The orgs with their rules, hosts and trackers (read-only).
    Orgs,
    /// Proposed orgs from owners and tracker sites (never applied).
    OrgSuggestions,
    /// The Today view's digest (work graph M9.1).
    Today,
    /// A ticket's context card from the cache (work graph M9.2).
    Card,
    /// One item's whole description, on demand: the cache, else one live
    /// fetch (Task 5 of the visible-truncation-and-describe plan).
    Describe,
    /// Tidy-up candidates (work graph M7).
    Tidy,
    /// Work open again that has past sessions (work graph M7).
    Reopened,
    /// Local work items: named work with no ticket (work graph M11.1).
    LocalItems,
    /// The Work view (work graph M14): a page of tasks with their sessions.
    Tree,
    /// One task with every session, its provenance and last outcome.
    Task,
    /// Every link of one session, with its task.
    SessionTasks,
    /// Suggestions and conflicts to decide.
    Review,
    /// Placement rules.
    Rules,
    /// What a drafted rule would move.
    RulePreview,
    /// Saved views.
    Views,
    /// What moving a local task to another org changes.
    OrgImpact,
    /// Sprints and releases in scope, with roll-ups (design 2026-09-28 §7).
    Buckets,
    /// One sprint or release with its members.
    Bucket,
    /// Missions in scope (orchestration O1).
    Missions,
    /// One mission with its items and latest events.
    Mission,
}

/// Every `work` action, by name — the ONLY place an action is parsed from,
/// so the org isolation matrix (`mcp::tools::tests_isolation`) can require a
/// row for each: a new action without one fails that test.
pub const WORK_ACTIONS: &[(&str, WorkAction)] = &[
    ("links", WorkAction::Links),
    ("context", WorkAction::Context),
    ("resume_plan", WorkAction::ResumePlan),
    ("purge_impact", WorkAction::PurgeImpact),
    ("tickets", WorkAction::Tickets),
    ("lookup", WorkAction::Lookup),
    ("trackers", WorkAction::Trackers),
    ("scopes", WorkAction::Scopes),
    ("orgs", WorkAction::Orgs),
    ("org_suggestions", WorkAction::OrgSuggestions),
    ("today", WorkAction::Today),
    ("card", WorkAction::Card),
    ("describe", WorkAction::Describe),
    ("tidy", WorkAction::Tidy),
    ("reopened", WorkAction::Reopened),
    ("local_items", WorkAction::LocalItems),
    ("tree", WorkAction::Tree),
    ("task", WorkAction::Task),
    ("session_tasks", WorkAction::SessionTasks),
    ("review", WorkAction::Review),
    ("rules", WorkAction::Rules),
    ("rule_preview", WorkAction::RulePreview),
    ("views", WorkAction::Views),
    ("org_impact", WorkAction::OrgImpact),
    ("buckets", WorkAction::Buckets),
    ("bucket", WorkAction::Bucket),
    ("missions", WorkAction::Missions),
    ("mission", WorkAction::Mission),
];

/// Every `work_link` action. The tool refuses any other name before
/// dispatching, so an action cannot be added without appearing here — and
/// so in the isolation matrix.
pub const WORK_LINK_ACTIONS: &[&str] = &[
    "link",
    "reject",
    "unlink",
    "switch",
    "confirm",
    "trust_project",
    "resume",
    "start",
    "preview_start",
    "abandon_start",
    "run",
    "handover",
    "archive",
    "unarchive",
    "snooze",
    "never",
    "dismiss",
    "tidy_apply",
    "create",
    "propose",
    "accept",
    "name",
    "set_status",
    "edit",
    "summarize",
    "set_primary",
    "reconsider",
    "ack",
    "decide_batch",
    "place",
    "assign_org",
    "rule_save",
    "rule_delete",
    "view_save",
    "view_delete",
    "bucket_add",
    "bucket_remove",
    "mission_save",
    "mission_state",
    "mission_repo",
    "mission_item",
    "mission_delete",
    "mission_import",
    "dep",
    "hold",
    "propose_tree",
    "accept_many",
    "undo_accept",
    "done_when",
    "verify",
    "mission_start",
    "mission_plan",
    "mission_grant",
    "mission_revoke",
    "retry",
    "card_decide",
    "missions_pause_all",
    "mission_release_note",
    "today_brief",
    "mission_triage",
];

/// The desktop's Routed work commands and the hub action each one calls
/// (`command`, `tool`, `action`). `src-tauri`'s routing tests hold this to
/// its verdict table, and the isolation matrix to the action lists above.
pub const ROUTED_WORK_COMMANDS: &[(&str, &str, &str)] = &[
    ("session_work_links", "work", "links"),
    ("work_resume_plan", "work", "resume_plan"),
    ("work_purge_impact", "work", "purge_impact"),
    ("list_trackers", "work", "trackers"),
    ("work_tickets", "work", "tickets"),
    ("work_lookup", "work", "lookup"),
    ("list_orgs", "work", "orgs"),
    ("org_suggestions", "work", "org_suggestions"),
    ("work_today", "work", "today"),
    ("work_ticket_card", "work", "card"),
    ("link_session_work", "work_link", "link"),
    ("reject_session_work", "work_link", "reject"),
    ("unlink_session_work", "work_link", "unlink"),
    ("switch_session_work", "work_link", "switch"),
    ("confirm_session_work", "work_link", "confirm"),
    ("set_work_project_trust", "work_link", "trust_project"),
    ("resume_work", "work_link", "resume"),
    ("start_work", "work_link", "start"),
    ("preview_start_work", "work_link", "preview_start"),
    ("abandon_start", "work_link", "abandon_start"),
    ("request_work_handover", "work_link", "handover"),
    ("start_work_multi", "work_link", "start"),
    ("work_tidy", "work", "tidy"),
    ("work_reopened", "work", "reopened"),
    ("unarchive_session_work", "work_link", "unarchive"),
    ("tidy_apply", "work_link", "tidy_apply"),
    ("dismiss_reopened", "work_link", "dismiss"),
    ("name_session_work", "work_link", "name"),
    ("rename_work_item", "work_link", "name"),
    ("create_work_task", "work_link", "create"),
    ("set_work_status", "work_link", "set_status"),
    ("edit_work_item", "work_link", "edit"),
    ("accept_work_proposal", "work_link", "accept"),
    ("reject_work_proposal", "work_link", "reject"),
    ("summarize_past_work", "work_link", "summarize"),
    // Work graph M14.1d: the Work view's desktop commands.
    ("work_tree", "work", "tree"),
    ("work_task", "work", "task"),
    ("work_session_tasks", "work", "session_tasks"),
    ("work_review", "work", "review"),
    ("work_rules", "work", "rules"),
    ("work_rule_preview", "work", "rule_preview"),
    ("work_views", "work", "views"),
    ("work_org_impact", "work", "org_impact"),
    ("set_primary_work", "work_link", "set_primary"),
    ("reconsider_work_link", "work_link", "reconsider"),
    ("ack_work_link", "work_link", "ack"),
    ("decide_work_batch", "work_link", "decide_batch"),
    ("place_work", "work_link", "place"),
    ("assign_work_org", "work_link", "assign_org"),
    ("save_work_rule", "work_link", "rule_save"),
    ("delete_work_rule", "work_link", "rule_delete"),
    ("save_work_view", "work_link", "view_save"),
    ("delete_work_view", "work_link", "view_delete"),
    // Sprints and releases (design 2026-09-28 §6a/§6b).
    ("work_buckets", "work", "buckets"),
    ("work_bucket", "work", "bucket"),
    ("add_work_to_bucket", "work_link", "bucket_add"),
    ("remove_work_from_bucket", "work_link", "bucket_remove"),
    // Orchestration O1: missions.
    ("work_missions", "work", "missions"),
    ("work_mission", "work", "mission"),
    ("save_mission", "work_link", "mission_save"),
    ("set_mission_state", "work_link", "mission_state"),
    ("set_mission_repo", "work_link", "mission_repo"),
    ("set_mission_item", "work_link", "mission_item"),
    ("delete_mission", "work_link", "mission_delete"),
    ("import_mission_plan", "work_link", "mission_import"),
    // Orchestration O2: the mission graph.
    ("set_work_dep", "work_link", "dep"),
    ("set_work_hold", "work_link", "hold"),
    ("accept_work_proposals", "work_link", "accept_many"),
    ("undo_work_accept", "work_link", "undo_accept"),
    // Orchestration O3: acceptance conditions and a person's check.
    ("set_work_done_when", "work_link", "done_when"),
    ("verify_work_item", "work_link", "verify"),
    ("start_mission_wave", "work_link", "mission_start"),
    ("retry_work_item", "work_link", "retry"),
    ("plan_mission", "work_link", "mission_plan"),
    ("decide_mission_card", "work_link", "card_decide"),
    ("grant_mission", "work_link", "mission_grant"),
    ("revoke_mission_grant", "work_link", "mission_revoke"),
    ("pause_all_missions", "work_link", "missions_pause_all"),
    // Redesign 9.11: LLM drafts in Control.
    ("mission_release_note", "work_link", "mission_release_note"),
    ("today_brief", "work_link", "today_brief"),
    ("mission_triage", "work_link", "mission_triage"),
];

/// The `action` schemas are generated from the tables above (work graph
/// M8.0), so a client that reads the `enum` — the phone, which draws a
/// button only for an action the hub serves — is offered exactly what the
/// parser and the dispatch accept.
fn action_schema<'a>(names: impl Iterator<Item = &'a str>) -> rmcp::schemars::Schema {
    let names: Vec<&str> = names.collect();
    rmcp::schemars::json_schema!({ "type": "string", "enum": names })
}

fn work_action_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    action_schema(WORK_ACTIONS.iter().map(|(n, _)| *n))
}

fn work_link_action_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    action_schema(WORK_LINK_ACTIONS.iter().copied())
}

/// The Work view's nested parameters (work graph M14) are served as a bare
/// `object` / `array`: their shape is in `docs/control-api.md` and the
/// spec, and a typed schema per nested struct would cost the tool budget
/// (C21) several kilobytes. The server still deserialises them strictly — a
/// malformed one is refused, never ignored.
fn object_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    rmcp::schemars::json_schema!({ "type": "object" })
}

fn array_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    rmcp::schemars::json_schema!({ "type": "array", "items": { "type": "object" } })
}

impl WorkArgs {
    pub fn parsed_action(&self) -> Result<WorkAction, IpcError> {
        let name = self.action.as_deref().unwrap_or("links");
        WORK_ACTIONS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, a)| *a)
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "unknown work action {name:?}; one of {}",
                        WORK_ACTIONS
                            .iter()
                            .map(|(n, _)| *n)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })
    }

    fn required_key(&self) -> Result<&str, IpcError> {
        self.key
            .as_deref()
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "this work action needs key"))
    }
}

/// `work { action: context, key }`.
pub async fn work_context(
    args: &WorkArgs,
    store: &Mutex<Store>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<WorkContext, IpcError> {
    let scope = &view.org;
    let key = crate::store::normalize_work_ref(args.required_key()?)?;
    orgs::require_key(&*lock(store)?, scope, &key)?;
    let input = handover::gather_handover(store, ssh.as_ref(), &key, None, view).await?;
    Ok(WorkContext {
        text: handover::build_context(&input),
        key,
    })
}

/// `work { action: resume_plan, key, link_id?, host_alias?, with_brief? }`.
pub async fn work_resume_plan(
    args: &WorkArgs,
    store: &Mutex<Store>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<resume::ResumePlan, IpcError> {
    resume::resume_plan(
        store,
        ssh,
        args.required_key()?,
        args.link_id,
        args.host_alias.as_deref(),
        args.with_brief.unwrap_or(false),
        view,
    )
    .await
}

/// `work { action: purge_impact, project_id, host_aliases }`.
///
/// A per-host token asks only about its own host, and hears only the keys
/// whose work it may read.
pub fn work_purge_impact(
    args: &WorkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<PurgeImpact, IpcError> {
    let pid = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "purge_impact needs project_id"))?;
    let hosts = args.host_aliases.clone().unwrap_or_default();
    if let Some(h) = scope.host() {
        if let Some(other) = hosts.iter().find(|x| x.as_str() != h) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!("the purge is on host {other}; this token is bound to {h}"),
            ));
        }
    }
    let s = lock(store)?;
    let mut keys = s.work_keys_for_purge(pid, &hosts)?;
    // This is the org boundary, not a privacy fence: which KEYS a purge may name. `PurgeImpact`
    // carries keys and no session row.
    //
    // **Open, and recorded as an owner decision** (multi-user M1, T9d; the
    // table row is in `scope_guard_tests::OPEN_QUESTIONS`). For a person's
    // own device `is_all()` is true, so no retention runs and `keys` is
    // every work key `Store::work_keys_for_purge` found on the project and
    // hosts — other people's included. A ticket key is a company's; a LOCAL
    // item's key is a sentence a person typed about their own work, which is
    // why this is still the owner's and not settled here.
    //
    // **It is the WHOLE of that residual** (multi-user M1, T10). T9b/T9c
    // attributed the same "key-level residual" to `orgs::require_key` /
    // `require_key_bound`, and T10 traced those: for a per-host token or an
    // org-bound client they answer an org question (does this key have work
    // in YOUR scope), and for every other caller — a person's own device
    // included — `require_key` returns `Ok(())` unconditionally and
    // discloses nothing. So this site is not "the third call site" of a
    // shared question; it is the only one, and `require_key`'s doc records
    // the decision that closes its half.
    //
    // Why it is not simply fenced by person here: the natural fix is to keep
    // a key only when the caller can see some link on it
    // (`orgs::scope_links_for`), and that would fence the OPERATOR too — the
    // master's `ViewScope` carries the personal owner as its person, so on a
    // multi-person hub the purge warning would stop naming the work the
    // purge is about to destroy, which §4.5 deliberately does not do to the
    // hub operator. Keys, never session rows.
    if !scope.is_all() {
        keys.retain(|k| orgs::require_key(&s, scope, k).is_ok());
    }
    Ok(PurgeImpact { keys })
}

/// `work_link { action: resume, key, mode, link_id?, host_alias?, brief? }`.
pub async fn work_resume(
    args: &WorkLinkArgs,
    store: &std::sync::Arc<Mutex<Store>>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    reg: &std::sync::Arc<crate::cancel::CancellationRegistry>,
    scope: &OrgScope,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<SessionRow, IpcError> {
    resume::resume_work(store, ssh, reg, &resume_args(args)?, scope, reader).await
}

/// `work { action: lookup }`'s reference: `url`, else `key`.
pub fn lookup_reference(args: &WorkArgs) -> Result<&str, IpcError> {
    args.url
        .as_deref()
        .or(args.key.as_deref())
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "lookup needs key or url"))
}

/// The start half of [`WorkLinkArgs`] (work graph M3.4): a PERSON's start
/// (the desktop's); an agent's goes through [`start_args_as`].
///
/// No owner: the desktop is one person at the keyboard and the hub's own
/// person is the right answer there (`hub_personal_owner`). The hub's own path
/// goes through [`start_args_owned`] with the CALLER's person.
pub fn start_args(args: &WorkLinkArgs) -> crate::service::trackers::tickets::StartArgs {
    start_args_as(args, Decider::Person)
}

/// [`start_args_as`] with the caller's person (multi-user M1, T5): whose the
/// started session is; and who started it (migration 124).
pub fn start_args_owned(
    args: &WorkLinkArgs,
    decider: Decider,
    owner: Option<i64>,
    origin: Option<crate::store::SessionOrigin>,
) -> crate::service::trackers::tickets::StartArgs {
    crate::service::trackers::tickets::StartArgs {
        owner,
        origin,
        ..start_args_as(args, decider)
    }
}

/// [`start_args`] started by `decider`, which decides the link's source:
/// `started` for a person, `agent_started` for an agent (D34).
pub fn start_args_as(
    args: &WorkLinkArgs,
    decider: Decider,
) -> crate::service::trackers::tickets::StartArgs {
    crate::service::trackers::tickets::StartArgs {
        decider,
        owner: None,
        origin: None,
        // A start names no login: `profile` is mission_grant's.
        profile: None,
        reference: args.url.clone().or(args.key.clone()),
        item_id: args.item_id,
        project_id: args.project_id,
        host_alias: args.host_alias.clone(),
        with_brief: args.with_brief.unwrap_or(false) || args.brief.is_some(),
        brief: args.brief.clone(),
        name: args.name.clone(),
        worktree: args.worktree.clone(),
        force_cross_org: args.force_cross_org.unwrap_or(false),
        per_project: false,
        parallel: args.parallel.unwrap_or(false),
    }
}

/// The resume half of [`WorkLinkArgs`].
pub fn resume_args(args: &WorkLinkArgs) -> Result<resume::ResumeArgs, IpcError> {
    Ok(resume::ResumeArgs {
        owner: None,
        key: args
            .key
            .clone()
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "resume needs key"))?,
        mode: args.mode.clone().unwrap_or_else(|| "last".into()),
        link_id: args.link_id,
        host_alias: args.host_alias.clone(),
        brief: args.brief.clone(),
        force_cross_org: args.force_cross_org.unwrap_or(false),
    })
}

/// Most recently ended links one `work {}` read returns.
pub const RECENT_LINKS_MAX: i64 = 200;

/// `{session_id}` → that session's live links (confirmed and rejected,
/// primary first); `{key}` → ended links to the key (past work); neither →
/// every link that ended recently. At most one of the two arguments.
/// A per-host token reads only links inside its orgs, and past links only of
/// its own host's sessions (`orgs::scope_links`).
///
/// Multi-user M1 (T8d): it takes the caller's WHOLE scope, not its org half.
/// Two of the three forms answer a PAGE — a key's ended links, and the
/// fleet-wide recent page a bare `work {}` returns — and a `WorkLinkRow`
/// carries `snap_tmux`, `snap_name`, `snap_branch`, `snap_worktree`,
/// `snap_pr_url` and `snap_claude_ids`, which spec §4.3 calls content. The
/// `{session_id}` form is gated per row in the handler as well, and the fence
/// here is harmless to it (the caller's own row is visible to the caller);
/// the two page forms had no person fence at all, because
/// `orgs::scope_links`' `OrgScope::All` arm is `{}` and `All` is what every
/// paired client bound to no org resolves to. T8's result gate could not net
/// them either: a `WorkLinkRow` spells the session `snap_host` / `snap_tmux`
/// and carries no `visibility`, so `looks_like_session_row` is false for it.
pub fn work(
    args: &WorkArgs,
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<WorkLinkRow>, IpcError> {
    let s = lock(store)?;
    let mut links = work_unscoped(args, &s)?;
    orgs::scope_links_for(&s, view, &mut links)?;
    Ok(links)
}

fn work_unscoped(args: &WorkArgs, s: &Store) -> Result<Vec<WorkLinkRow>, IpcError> {
    match (args.session_id, args.key.as_deref()) {
        (Some(id), None) => s.session_work_links(id),
        (None, Some(key)) => s.ended_work_links_for_key(key),
        // Neither: work that ended recently (`work.recent_days`), so past-only
        // work has a group to show in.
        (None, None) => {
            let days = crate::service::settings::resolve(
                crate::service::settings::WORK_RECENT_DAYS,
                s.get_setting(crate::service::settings::WORK_RECENT_DAYS)?
                    .as_deref(),
            )
            .parse::<i64>()
            .unwrap_or(14);
            s.recent_ended_work_links(
                crate::service::catalog::now_secs() - days * 86_400,
                RECENT_LINKS_MAX,
            )
        }
        (Some(_), Some(_)) => Err(IpcError::new(
            codes::E_INVALID,
            "pass at most one of session_id or key",
        )),
    }
}

/// Apply one link decision and return the session's updated row (its `work`
/// is the new primary link, or none).
///
/// Under a per-host token's scope (work graph M5) the target must be inside
/// its orgs: an item id or link id outside answers exactly as an unknown one,
/// a key as a key nothing is linked to. For every caller, linking or
/// confirming work of one org on a session of another is refused unless
/// `force_cross_org` ([`orgs::check_cross_org`]).
///
/// A PERSON's decision (the desktop's commands); an agent's goes through
/// [`work_link_as`].
pub fn work_link(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<SessionRow, IpcError> {
    work_link_as(args, store, scope, Decider::Person)
}

/// [`work_link`] decided by `decider`, which decides the source a link,
/// confirm or reject records: an agent's is `agent` whatever `source` it
/// passed (only `agent_inferred`, a suggestion, is kept), so it never reads
/// as a person's decision; a person's `link` keeps its `source` (default
/// `manual`). An agent cannot overturn a person's rejection (`E_FORBIDDEN`).
pub fn work_link_as(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    decider: Decider,
) -> Result<SessionRow, IpcError> {
    let s = lock(store)?;
    work_link_locked(args, &s, scope, decider)
}

/// What a decision on one link by id answers: the session's row, and
/// (additively, on the wire) that link's version after the write — read
/// under the same lock, so a client's next compare-and-set (the Review's
/// Undo) names it without re-reading the session's links and racing
/// another device. `link_version` is absent for any other action, when
/// the link is no longer one of the session's live links, and from a hub
/// built before it (a client then re-reads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecidedRow {
    #[serde(flatten)]
    pub row: SessionRow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_version: Option<i64>,
}

/// [`work_link_as`] answering a [`DecidedRow`]: `link_version` is set for
/// `confirm` and `reject { link_id }`.
pub fn work_link_decided(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    decider: Decider,
) -> Result<DecidedRow, IpcError> {
    let s = lock(store)?;
    let row = work_link_locked(args, &s, scope, decider)?;
    let decided = match args.action.as_str() {
        "confirm" => args.link_id,
        "reject" if args.key.is_none() && args.item_id.is_none() => args.link_id,
        _ => None,
    };
    // The link was checked visible to the caller before the write; it must
    // still be one of this session's live links to be named.
    let link_version = match (decided, args.session_id) {
        (Some(id), Some(sid)) if s.session_work_links(sid)?.iter().any(|l| l.id == id) => {
            s.work_link_version(id)?
        }
        _ => None,
    };
    Ok(DecidedRow { row, link_version })
}

fn work_link_locked<'a>(
    args: &'a WorkLinkArgs,
    s: &Store,
    scope: &OrgScope,
    decider: Decider,
) -> Result<SessionRow, IpcError> {
    if matches!(
        args.action.as_str(),
        "resume"
            | "start"
            | "run"
            | "trust_project"
            | "handover"
            | "dismiss"
            | "tidy_apply"
            | "name"
            | "decide_batch"
            | "place"
            | "assign_org"
            | "rule_save"
            | "rule_delete"
            | "view_save"
            | "view_delete"
    ) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{} has its own entry point", args.action),
        ));
    }
    let session_id = args.session_id.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{} needs session_id", args.action),
        )
    })?;
    // A client bound to an org never reaches another org's session (the
    // transport's session gate says so first; this keeps the service's own
    // answer the same — the unknown session's — for every entry point, the
    // batch's included).
    if matches!(scope, OrgScope::Org { .. })
        && !s
            .get_session_by_id(session_id)?
            // The org half. The person half is the transport's session gate
            // (`resolve_row_person_gated`), which every `work_link` entry
            // point takes before reaching here; this keeps the SERVICE's own
            // answer the same for the batch path.
            .is_some_and(|row| scope.sees_row_org_only(&row))
    {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("session {session_id} not found"),
        ));
    }
    let force = args.force_cross_org.unwrap_or(false);
    // The target's org, checked against the scope (visibility) before
    // anything is written.
    // An item id outside the scope answers as an unknown id. A KEY outside
    // it links as the bare key it is to this caller (`WorkTarget::Ref`) —
    // exactly what an unknown key does — so neither answer says the other
    // org has it.
    let visible_target = |t: WorkTarget<'a>| -> Result<(WorkTarget<'a>, Option<i64>), IpcError> {
        let org = s.work_target_org(t)?;
        if scope.sees_org(org) {
            return Ok((t, org));
        }
        match t {
            WorkTarget::Key(k) | WorkTarget::Ref(k) => Ok((WorkTarget::Ref(k), None)),
            WorkTarget::Item(id) => Err(orgs::not_found("work item", id)),
        }
    };
    // A link id must be one of this session's live links, and visible.
    let visible_link = |link_id: i64| -> Result<WorkLinkRow, IpcError> {
        let mut l = s
            .session_work_links(session_id)?
            .into_iter()
            .find(|l| l.id == link_id)
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} has no live work link {link_id}"),
                )
            })?;
        l.org_id = s.link_org(&l)?;
        if !scope.sees_link(&l) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} has no live work link {link_id}"),
            ));
        }
        Ok(l)
    };
    // The live link a tidy flag goes to — `link_id`, else the primary —
    // resolved BEFORE the visibility check, so that omitting `link_id` never
    // reaches a primary the scope does not see (a forced cross-org link on
    // the caller's own session). Such a session answers as one with no
    // linked work, as the store does for a session without any.
    let tidy_target = || -> Result<i64, IpcError> {
        let link_id = s.tidy_link(session_id, args.link_id)?;
        // This is the org boundary, not a privacy fence: the session was already reached through
        // the tool layer's `Reach::Drive` person gate; what is left to ask is whether the LINK
        // is this caller's org's.
        if !scope.is_all() && visible_link(link_id).is_err() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                match args.link_id {
                    Some(l) => format!("session {session_id} has no live work link {l}"),
                    None => format!("session {session_id} has no linked work"),
                },
            ));
        }
        Ok(link_id)
    };
    // The lifecycle actions (work graph M7) write flags, not decisions: no
    // resolver run after them.
    match args.action.as_str() {
        "archive" => {
            // A per-host token stamps only the links it sees.
            // This is the org boundary, not a privacy fence: which of an already-reached
            // session's links an archive stamps, by org.
            let only = if scope.is_all() {
                None
            } else {
                let mut links = s.session_work_links(session_id)?;
                s.fill_link_orgs(&mut links)?;
                Some(
                    links
                        .iter()
                        .filter(|l| scope.sees_link(l))
                        .map(|l| l.id)
                        .collect::<Vec<i64>>(),
                )
            };
            s.archive_session_links(session_id, only.as_deref())?;
            return lifecycle_row(s, session_id);
        }
        // A click, or an attach: a person's touch un-archives.
        "unarchive" => {
            if !s.touch_session(session_id)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} not found"),
                ));
            }
            return lifecycle_row(s, session_id);
        }
        "snooze" => {
            let link_id = tidy_target()?;
            let days = args.days.unwrap_or(tidy::SNOOZE_DEFAULT_DAYS);
            if !(1..=tidy::SNOOZE_MAX_DAYS).contains(&days) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("snooze days must be 1..={}", tidy::SNOOZE_MAX_DAYS),
                ));
            }
            let until = crate::service::catalog::now_secs() + i64::from(days) * 86_400;
            s.snooze_tidy(session_id, Some(link_id), until)?;
            return lifecycle_row(s, session_id);
        }
        "never" => {
            let link_id = tidy_target()?;
            s.never_tidy(session_id, Some(link_id))?;
            return lifecycle_row(s, session_id);
        }
        _ => {}
    }
    // Work graph M14.1c: a decision on a link by id names the version the
    // person saw; someone else's change meanwhile is `E_CONFLICT` with the
    // link's current state. Checked only AFTER the link is known to be one
    // the caller may name (this session's live link, in its scope): a
    // version or state is never an oracle for a link out of scope. The
    // store lock is held from here to the write, so the check and the
    // write are one step.
    let checked_link = |link_id: i64| -> Result<(), IpcError> {
        visible_link(link_id)?;
        s.check_link_version(link_id, args.expected_version)
    };
    let take_primary = args.primary.unwrap_or(true);
    match args.action.as_str() {
        // Move the primary (compare-and-set), keep a conflict, undo a
        // decision.
        "set_primary" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_primary needs link_id"))?;
            checked_link(link_id)?;
            // The compare-and-set is on the primary the CALLER can see: a
            // scoped caller whose session's primary is another org's (a
            // forced link) saw "none", and must neither be refused forever
            // nor be told that link's id. The store's own check then runs
            // on the actual primary, under this same lock.
            let actual = s.current_primary_link(session_id)?;
            // This is the org boundary, not a privacy fence: the same org question about the
            // primary link.
            let seen = actual.filter(|id| scope.is_all() || visible_link(*id).is_ok());
            if let Some(expected) = args.expected_primary {
                if seen.unwrap_or(0) != expected {
                    return Err(crate::store::primary_conflict(session_id, seen));
                }
            }
            s.set_primary_work_link(session_id, link_id, Some(actual.unwrap_or(0)))?;
            return lifecycle_row(s, session_id);
        }
        // D32: a person keeps a conflict (a forced cross-org link, a link
        // to an unavailable ticket); the review inbox stops listing it.
        "ack" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "ack needs link_id"))?;
            checked_link(link_id)?;
            s.ack_work_link(session_id, link_id)?;
            return lifecycle_row(s, session_id);
        }
        // Undo: a person's confirm / reject goes back to a suggestion. A
        // reconsidered primary frees the primary: the resolver below picks
        // the next one.
        "reconsider" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "reconsider needs link_id"))?;
            checked_link(link_id)?;
            s.reconsider_work_link(session_id, link_id, decider)?;
        }
        _ => {}
    }
    let target = || -> Result<WorkTarget<'_>, IpcError> {
        match (args.item_id, args.key.as_deref()) {
            (Some(id), None) => Ok(WorkTarget::Item(id)),
            (None, Some(key)) => Ok(WorkTarget::Key(key)),
            _ => Err(IpcError::new(
                codes::E_INVALID,
                format!("{} needs exactly one of key or item_id", args.action),
            )),
        }
    };
    match args.action.as_str() {
        // Task → session P-2: end the live link `link_id` and make the
        // target the primary, in one compare-and-set on the primary.
        "switch" => {
            let from = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "switch needs link_id"))?;
            visible_link(from)?;
            let source = link_source(None, decider)?;
            let (t, org) = visible_target(target()?)?;
            orgs::check_cross_org(org, s.session_org(session_id)?, &target_name(t), force)?;
            // The compare-and-set is on the primary the caller can see, as
            // in `set_primary`: another org's primary reads as none and its
            // id is never named in the refusal.
            let actual = s.current_primary_link(session_id)?;
            // This is the org boundary, not a privacy fence: the same org question about the
            // primary link.
            let seen = actual.filter(|id| scope.is_all() || visible_link(*id).is_ok());
            if let Some(expected) = args.expected_primary {
                if seen.unwrap_or(0) != expected {
                    return Err(crate::store::primary_conflict(session_id, seen));
                }
            }
            s.switch_session_work(session_id, from, t, source, Some(actual.unwrap_or(0)))?;
        }
        "link" => {
            let source = link_source(args.source.as_deref(), decider)?;
            let (t, org) = visible_target(target()?)?;
            orgs::check_cross_org(org, s.session_org(session_id)?, &target_name(t), force)?;
            if source == AGENT_INFERRED {
                // The classification nudge's answer (work graph M4.6): a
                // guess, so a pre-selected suggestion (R11), never a link.
                let (key, tracker) = inferred_target(s, t)?;
                detect::on_agent_inference(s, session_id, &key, tracker)?;
            } else {
                s.link_session_work_as(session_id, t, source, take_primary, args.expected_version)?;
            }
        }
        // `reject { link_id }` decides one suggestion (work graph M4.4);
        // `reject { key | item_id }` any target.
        "reject" if args.link_id.is_some() && args.key.is_none() && args.item_id.is_none() => {
            let link_id = args.link_id.unwrap_or_default();
            // This is the org boundary, not a privacy fence: the same org question before a
            // reject by link id.
            if !scope.is_all() || args.expected_version.is_some() {
                checked_link(link_id)?;
            }
            detect::decide(s, session_id, link_id, false, decider)?;
        }
        "reject" => {
            let (t, _) = visible_target(target()?)?;
            s.reject_session_work_as(session_id, t, decider, args.expected_version)?;
        }
        "confirm" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "confirm needs link_id"))?;
            if let Ok(l) = visible_link(link_id) {
                // Confirming a guess makes it a link: the same integrity rule.
                let org = l.item_id.map(|i| s.item_org(i)).transpose()?.flatten();
                orgs::check_cross_org(
                    org,
                    s.session_org(session_id)?,
                    &format!("work link {link_id}"),
                    force,
                )?;
            // This is the org boundary, not a privacy fence: the same org question before a
            // confirm by link id.
            } else if !scope.is_all() {
                visible_link(link_id)?;
            }
            if args.expected_version.is_some() {
                checked_link(link_id)?;
            }
            detect::decide_as(s, session_id, link_id, true, take_primary, decider)?;
        }
        "unlink" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "unlink needs link_id"))?;
            // This is the org boundary, not a privacy fence: the same org question before an
            // unlink by link id.
            if !scope.is_all() || args.expected_version.is_some() {
                checked_link(link_id)?;
            }
            // A person's "Clear work" holds against the unchanged branch /
            // PR that named the target, or the re-resolve below would make
            // the same link again (R9u). An agent's unlink stays a plain
            // unlink: it never writes a hold a person did not ask for.
            let holds = match decider {
                Decider::Person => detect::unlink_holds(s, session_id, link_id)?,
                Decider::Agent => Vec::new(),
            };
            // D34: removing a person's rejection ("Not this") would let the
            // agent's next link — or detection — make it again: unlink, then
            // link, is the two-step overturn `confirm` already refuses.
            if decider == Decider::Agent {
                let own = s.session_work_links(session_id)?;
                if let Some(l) = own.iter().find(|l| l.id == link_id) {
                    if l.state == "rejected" && PERSON_SOURCES.contains(&l.source.as_str()) {
                        return Err(IpcError::new(
                            codes::E_FORBIDDEN,
                            format!(
                                "a person rejected this work for session {session_id} (work \
                                 link {link_id}); an agent cannot remove a person's rejection"
                            ),
                        )
                        .with_details(serde_json::json!({
                            "link_id": link_id,
                            "reason": "rejected_by_person",
                        })));
                    }
                }
            }
            if !s.unlink_session_work_held(session_id, link_id, &holds)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} has no live work link {link_id}"),
                ));
            }
        }
        // Applied above; the resolver below runs after it.
        "reconsider" => {}
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown work_link action {other:?}; one of {}",
                    WORK_LINK_ACTIONS.join(", ")
                ),
            ))
        }
    }
    // A decision can leave a sole candidate or free a primary (M4.3).
    if let Err(e) = detect::resolve_session(s, session_id) {
        tracing::debug!(error = %e.message, "[work] resolve after a decision failed");
    }
    // A link made after the PR: the probe queues only when the PR's signals
    // change, so queue its write-back here (M13.4e). `on_pr` keeps it to a
    // person's confirmed link; the outbox makes a repeat a no-op.
    if matches!(args.action.as_str(), "link" | "switch" | "confirm") {
        if let Err(e) = crate::service::trackers::write_back::on_session_pr(s, session_id) {
            tracing::debug!(error = %e.message, "[write-back] not queued after a link");
        }
    }
    s.get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

/// One session already on the task a `link` or `switch` names (task →
/// session P-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveElsewhere {
    /// Named only when the caller may see the session (D7, T9b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// One plain sentence for the person.
    pub message: String,
}

/// Task → session P-3: a `link` or `switch` that asks for the warning
/// (`ack_live: false`) is refused with `E_EXISTS` and
/// `details.live_elsewhere[]` when the task already has another live
/// session; the person then sends it again with `ack_live: true`.
/// Absent `ack_live` checks nothing, so an older client, the phone and the
/// agents link exactly as before. Only sessions in the caller's orgs are
/// counted, so the refusal never says another org works on a key; one the
/// caller may not see is "someone", never named.
pub fn check_live_elsewhere(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<(), IpcError> {
    if args.ack_live != Some(false)
        || !matches!(args.action.as_str(), "link" | "switch")
        || args.source.as_deref() == Some(AGENT_INFERRED)
    {
        return Ok(());
    }
    let Some(session_id) = args.session_id else {
        return Ok(());
    };
    let target = match (args.item_id, args.key.as_deref()) {
        (Some(id), None) => WorkTarget::Item(id),
        (None, Some(key)) => WorkTarget::Key(key),
        // The action itself refuses the shape.
        _ => return Ok(()),
    };
    let s = lock(store)?;
    // The target as the action will read it: an item outside the scope
    // answers as unknown there, so it is not looked at here; a key outside
    // it is the bare key it is to this caller.
    let target = match s.work_target_org(target) {
        Ok(org) if view.org.sees_org(org) => target,
        Ok(_) => match target {
            WorkTarget::Key(k) | WorkTarget::Ref(k) => WorkTarget::Ref(k),
            WorkTarget::Item(_) => return Ok(()),
        },
        Err(e) if matches!(e.code.as_str(), codes::E_NOTFOUND | codes::E_INVALID) => return Ok(()),
        Err(e) => return Err(e),
    };
    let ids = match s.live_sessions_on_target(target, session_id) {
        Ok(ids) => ids,
        // An unknown item or a malformed key: the action answers it.
        Err(e) if matches!(e.code.as_str(), codes::E_NOTFOUND | codes::E_INVALID) => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for id in ids {
        let Some(row) = s.get_session_by_id(id)? else {
            continue;
        };
        // The org half alone decides whether the session COUNTS: a task's
        // other live session in the caller's org is worth a warning even
        // when the caller may not see it. The person half,
        // `sees_session_row`, decides only whether it is NAMED.
        if !view.org.sees_row_org_only(&row) {
            continue;
        }
        let visible = view.sees_session_row(&row).is_visible();
        found.push(LiveElsewhere {
            session_id: visible.then_some(row.id),
            message: if visible {
                format!(
                    "Already open in {} on {}.",
                    row.friendly_name.as_deref().unwrap_or(&row.tmux_name),
                    row.host_alias
                )
            } else {
                "Someone is already working on this.".into()
            },
        });
    }
    if found.is_empty() {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_EXISTS,
        format!(
            "this task already has {} live session{}; send again with ack_live: true to \
             {} anyway",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            args.action
        ),
    )
    .with_details(serde_json::json!({ "live_elsewhere": found })))
}

fn lifecycle_row(s: &Store, session_id: i64) -> Result<SessionRow, IpcError> {
    s.get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

/// `work_link { action: link, source }`'s value for the agent's answer to
/// the classification nudge (work graph M4.6).
pub const AGENT_INFERRED: &str = "agent_inferred";

/// The source a `link` records. A person's `source` is kept (default
/// `manual`); an agent's is `agent` whatever it claimed — `manual` or
/// `started` would read as a person's decision (usage, write-back) —
/// except `agent_inferred`, which is a suggestion, not a link. An unknown
/// source is refused either way.
fn link_source(requested: Option<&str>, decider: Decider) -> Result<&str, IpcError> {
    let requested = requested.unwrap_or(decider.source());
    if requested == AGENT_INFERRED {
        return Ok(AGENT_INFERRED);
    }
    if !crate::store::WORK_LINK_SOURCES.contains(&requested) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "unknown work link source {requested:?}; one of {}, {AGENT_INFERRED}",
                crate::store::WORK_LINK_SOURCES.join(", ")
            ),
        ));
    }
    Ok(match decider {
        Decider::Person => requested,
        Decider::Agent => Decider::Agent.source(),
    })
}

/// The resolver target an agent inference names: the key (normalised) and,
/// for an item, its tracker. A keyless item cannot be one — the resolver
/// addresses work by key.
fn inferred_target(s: &Store, t: WorkTarget<'_>) -> Result<(String, Option<i64>), IpcError> {
    match t {
        WorkTarget::Key(k) | WorkTarget::Ref(k) => Ok((crate::store::normalize_work_ref(k)?, None)),
        WorkTarget::Item(id) => {
            let item = s
                .get_work_item(id)?
                .ok_or_else(|| orgs::not_found("work item", id))?;
            let key = item.key.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!("work item {id} has no key; an inference names work by its key"),
                )
            })?;
            Ok((crate::store::normalize_work_ref(&key)?, item.tracker_id))
        }
    }
}

/// A target as a sentence names it.
fn target_name(t: WorkTarget<'_>) -> String {
    match t {
        WorkTarget::Item(id) => format!("work item {id}"),
        WorkTarget::Key(k) | WorkTarget::Ref(k) => k.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("dev", "h", None, None, 1, 1, "running", None)
            .unwrap();
        (Mutex::new(s), id)
    }

    /// The served schema's `enum` is the table, in order — what the phone
    /// reads to decide which buttons to draw (work graph M8.0).
    #[test]
    fn the_action_schemas_enumerate_the_tables() {
        let w = serde_json::to_value(rmcp::schemars::schema_for!(WorkArgs)).unwrap();
        let work: Vec<&str> = WORK_ACTIONS.iter().map(|(n, _)| *n).collect();
        assert_eq!(w["properties"]["action"]["enum"], serde_json::json!(work));
        let l = serde_json::to_value(rmcp::schemars::schema_for!(WorkLinkArgs)).unwrap();
        assert_eq!(
            l["properties"]["action"]["enum"],
            serde_json::json!(WORK_LINK_ACTIONS)
        );
        assert_eq!(l["required"], serde_json::json!(["action"]));
    }

    fn link(sid: i64, action: &str) -> WorkLinkArgs {
        WorkLinkArgs {
            session_id: Some(sid),
            action: action.into(),
            ..Default::default()
        }
    }

    /// Task → session P-2 through the service: the primary moves, the old
    /// link ends, and a stale view of the primary is a conflict.
    #[test]
    fn switch_moves_the_primary_in_one_step() {
        let (st, sid) = store();
        let a = WorkLinkArgs {
            key: Some("ABC-1".into()),
            ..link(sid, "link")
        };
        work_link(&a, &st, &OrgScope::All).unwrap();
        let from = lock(&st)
            .unwrap()
            .current_primary_link(sid)
            .unwrap()
            .unwrap();
        let stale = WorkLinkArgs {
            key: Some("DEF-2".into()),
            link_id: Some(from),
            expected_primary: Some(from + 1),
            ..link(sid, "switch")
        };
        assert_eq!(
            work_link(&stale, &st, &OrgScope::All).unwrap_err().code,
            codes::E_CONFLICT
        );
        let row = work_link(
            &WorkLinkArgs {
                expected_primary: Some(from),
                ..stale
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        let w = row.work.expect("the row carries its work");
        assert_eq!(w.key.as_deref(), Some("DEF-2"));
        let s = lock(&st).unwrap();
        let old = s.get_work_link(from).unwrap().unwrap();
        assert_eq!(old.end_reason.as_deref(), Some("switched"));
        assert_eq!(s.session_work_links(sid).unwrap().len(), 1);
    }

    /// Task → session P-3: asked for, the warning names the other live
    /// session; acknowledged, or not asked for, the link goes ahead.
    #[test]
    fn a_link_asked_to_warn_names_the_tasks_other_live_session() {
        let (st, sid) = store();
        let other = {
            let s = lock(&st).unwrap();
            let o = s
                .upsert_session("other", "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.link_session_work(o, WorkTarget::Key("ABC-1"), "manual")
                .unwrap();
            o
        };
        let view = crate::service::view_scope::ViewScope::internal();
        let ask = |ack: Option<bool>| WorkLinkArgs {
            key: Some("abc-1".into()),
            ack_live: ack,
            ..link(sid, "link")
        };
        let e = check_live_elsewhere(&ask(Some(false)), &st, &view).unwrap_err();
        assert_eq!(e.code, codes::E_EXISTS);
        let d = e.details.unwrap();
        assert_eq!(d["live_elsewhere"][0]["session_id"], other);
        assert!(d["live_elsewhere"][0]["message"]
            .as_str()
            .unwrap()
            .contains("other"));
        check_live_elsewhere(&ask(Some(true)), &st, &view).unwrap();
        check_live_elsewhere(&ask(None), &st, &view).unwrap();
        // Nothing else on the work: nothing to say.
        check_live_elsewhere(
            &WorkLinkArgs {
                key: Some("XYZ-9".into()),
                ..ask(Some(false))
            },
            &st,
            &view,
        )
        .unwrap();
    }

    #[test]
    fn link_reject_and_unlink_answer_the_updated_row() {
        let (st, sid) = store();
        let row = work_link(
            &WorkLinkArgs {
                key: Some("abc-1".into()),
                source: Some("agent".into()),
                ..link(sid, "link")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        let w = row.work.expect("primary work");
        assert_eq!(
            (w.key.as_deref(), w.source.as_str()),
            (Some("ABC-1"), "agent")
        );

        let links = work(
            &WorkArgs {
                session_id: Some(sid),
                ..Default::default()
            },
            &st,
            &crate::service::view_scope::ViewScope::internal(),
        )
        .unwrap();
        assert_eq!(links.len(), 1);

        let row = work_link(
            &WorkLinkArgs {
                link_id: Some(w.link_id),
                ..link(sid, "unlink")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(row.work, None);
        let err = work_link(
            &WorkLinkArgs {
                link_id: Some(w.link_id),
                ..link(sid, "unlink")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);

        let row = work_link(
            &WorkLinkArgs {
                key: Some("ABC-1".into()),
                ..link(sid, "reject")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(row.work, None);
        let links = work(
            &WorkArgs {
                session_id: Some(sid),
                ..Default::default()
            },
            &st,
            &crate::service::view_scope::ViewScope::internal(),
        )
        .unwrap();
        assert_eq!(links[0].state, "rejected");
    }

    /// D34: the decider, not the `source` a caller passes, decides what a
    /// link, confirm or reject records. An agent's is `agent` (its
    /// `agent_inferred` stays a suggestion); a person's keeps its source.
    #[test]
    fn the_decider_decides_the_recorded_source() {
        let (st, sid) = store();
        let key = |k: &str, action: &str, source: Option<&str>| WorkLinkArgs {
            key: Some(k.into()),
            source: source.map(String::from),
            ..link(sid, action)
        };
        let src = |row: SessionRow| row.work.map(|w| w.source).unwrap_or_default();
        // An agent claiming a person's source is still an agent.
        for claimed in [None, Some("manual"), Some("started"), Some("agent")] {
            let row = work_link_as(
                &key("ABC-1", "link", claimed),
                &st,
                &OrgScope::All,
                Decider::Agent,
            )
            .unwrap();
            assert_eq!(src(row), "agent", "{claimed:?}");
        }
        // An unknown source is refused for an agent too.
        let err = work_link_as(
            &key("ABC-1", "link", Some("branch")),
            &st,
            &OrgScope::All,
            Decider::Agent,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        // A person's source is kept (default manual).
        for (claimed, want) in [(None, "manual"), (Some("started"), "started")] {
            let row = work_link(&key("DEF-2", "link", claimed), &st, &OrgScope::All).unwrap();
            assert_eq!(src(row), want, "{claimed:?}");
        }
        // An agent's rejection by key is the agent's.
        work_link_as(
            &key("GHI-3", "reject", None),
            &st,
            &OrgScope::All,
            Decider::Agent,
        )
        .unwrap();
        let links = st.lock().unwrap().session_work_links(sid).unwrap();
        let ghi = links
            .iter()
            .find(|l| l.ref_key.as_deref() == Some("GHI-3"))
            .unwrap();
        assert_eq!(
            (ghi.state.as_str(), ghi.source.as_str()),
            ("rejected", "agent")
        );
    }

    /// D34: by link id, an agent's confirm is recorded as the agent's, it
    /// cannot confirm what a person rejected, and deciding the way a person
    /// already did keeps the person's decision.
    #[test]
    fn an_agent_decides_a_suggestion_as_an_agent_and_never_over_a_person() {
        let (st, sid) = store();
        let suggestion = |key: &str| -> i64 {
            let s = st.lock().unwrap();
            let l = s
                .link_session_work(sid, WorkTarget::Key(key), "manual")
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE work_links SET state = 'suggested', source = 'prompt', \
                       rule = 'R6', is_primary = 0, decided_at = NULL WHERE id = ?1",
                    [l.id],
                )
                .unwrap();
            l.id
        };
        let by_id = |id: i64, action: &str| WorkLinkArgs {
            link_id: Some(id),
            ..link(sid, action)
        };
        let get = |id: i64| st.lock().unwrap().get_work_link(id).unwrap().unwrap();

        // An agent's confirm reads as the agent's.
        let a = suggestion("ABC-1");
        let row = work_link_as(&by_id(a, "confirm"), &st, &OrgScope::All, Decider::Agent).unwrap();
        assert_eq!(row.work.map(|w| w.source).as_deref(), Some("agent"));
        assert_eq!(get(a).rule.as_deref(), Some("R6"), "the why is kept");

        // A person's "Not this" stands against an agent's confirm…
        let b = suggestion("DEF-2");
        work_link(&by_id(b, "reject"), &st, &OrgScope::All).unwrap();
        let err =
            work_link_as(&by_id(b, "confirm"), &st, &OrgScope::All, Decider::Agent).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        // …and an agent rejecting it again leaves it the person's.
        let before = get(b);
        work_link_as(&by_id(b, "reject"), &st, &OrgScope::All, Decider::Agent).unwrap();
        assert_eq!(get(b), before);
        // So does an agent's link by key.
        let err = work_link_as(
            &WorkLinkArgs {
                key: Some("DEF-2".into()),
                ..link(sid, "link")
            },
            &st,
            &OrgScope::All,
            Decider::Agent,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);

        // A person confirmed it: an agent's confirm makes it primary again
        // and leaves it the person's decision.
        let c = suggestion("GHI-3");
        work_link(&by_id(c, "confirm"), &st, &OrgScope::All).unwrap();
        let person = get(c);
        assert_eq!(person.source, "manual");
        work_link_as(&by_id(a, "confirm"), &st, &OrgScope::All, Decider::Agent).unwrap();
        assert!(!get(c).is_primary);
        let row = work_link_as(&by_id(c, "confirm"), &st, &OrgScope::All, Decider::Agent).unwrap();
        let w = row.work.expect("primary");
        assert_eq!((w.link_id, w.source.as_str()), (c, "manual"));
        assert_eq!(get(c).decided_at, person.decided_at);

        // A person may still correct their own rejection.
        work_link(&by_id(b, "confirm"), &st, &OrgScope::All).unwrap();
        assert_eq!(get(b).state, "confirmed");
    }

    /// A decision on one link by id answers that link's version after the
    /// write (the Review's Undo names it); any other action answers none,
    /// and the wire row stays a plain session row plus the one key.
    #[test]
    fn a_decision_by_link_id_answers_the_links_new_version() {
        let (st, sid) = store();
        let suggestion = |key: &str| -> i64 {
            let s = st.lock().unwrap();
            let l = s
                .link_session_work(sid, WorkTarget::Key(key), "manual")
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE work_links SET state = 'suggested', source = 'prompt', \
                       rule = 'R6', is_primary = 0, decided_at = NULL WHERE id = ?1",
                    [l.id],
                )
                .unwrap();
            l.id
        };
        let by_id = |id: i64, action: &str| WorkLinkArgs {
            link_id: Some(id),
            ..link(sid, action)
        };
        let version = |id: i64| st.lock().unwrap().work_link_version(id).unwrap();
        for action in ["confirm", "reject"] {
            let id = suggestion(if action == "confirm" {
                "ABC-1"
            } else {
                "DEF-2"
            });
            let before = version(id).unwrap();
            let d = work_link_decided(&by_id(id, action), &st, &OrgScope::All, Decider::Person)
                .unwrap();
            let after = version(id).unwrap();
            assert!(after > before, "{action}: the write moved it");
            assert_eq!(d.link_version, Some(after), "{action}");
            assert_eq!(d.row.id, sid);
            let wire = serde_json::to_value(&d).unwrap();
            assert_eq!(wire["link_version"], after);
            assert_eq!(wire["tmux_name"], "dev", "flattened: still a session row");
            // A client reading it as a plain row (an older desktop) still can.
            let plain: SessionRow = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(plain, d.row);
            let back: DecidedRow = serde_json::from_value(wire).unwrap();
            assert_eq!(back, d);
        }
        // Not a decision on one link by id: nothing to name.
        let d = work_link_decided(
            &WorkLinkArgs {
                key: Some("GHI-3".into()),
                ..link(sid, "link")
            },
            &st,
            &OrgScope::All,
            Decider::Person,
        )
        .unwrap();
        assert_eq!(d.link_version, None);
        assert!(serde_json::to_value(&d)
            .unwrap()
            .get("link_version")
            .is_none());
    }

    /// Work graph M4.6: an agent's answer to the classification nudge is a
    /// pre-selected suggestion (R11), never the session's work; a person's
    /// rejection is final (R9); and confirming it makes it a link.
    #[test]
    fn an_agent_inference_is_a_preselected_suggestion_a_person_decides() {
        let (st, sid) = store();
        {
            let s = st.lock().unwrap();
            s.create_local_work_item(Some("PAY-7"), "Retry").unwrap();
        }
        let infer = |key: &str| WorkLinkArgs {
            key: Some(key.into()),
            source: Some(AGENT_INFERRED.into()),
            ..link(sid, "link")
        };

        let row = work_link(&infer("pay-7"), &st, &OrgScope::All).unwrap();
        assert_eq!(row.work, None, "a guess never becomes the session's work");
        let g = row.work_suggested.expect("the inference is suggested");
        assert_eq!(g.key.as_deref(), Some("PAY-7"));
        assert_eq!(g.source, AGENT_INFERRED);
        assert_eq!(g.strength.as_deref(), Some("inferred"));
        assert_eq!(g.rule.as_deref(), Some("R11"));
        assert!(g.preselected, "shown ticked");

        // Said twice, it is still one suggestion.
        let again = work_link(&infer("PAY-7"), &st, &OrgScope::All).unwrap();
        assert_eq!(again.work_suggested.map(|w| w.link_id), Some(g.link_id));

        // Rejected by the person: the agent cannot bring it back.
        work_link(
            &WorkLinkArgs {
                link_id: Some(g.link_id),
                ..link(sid, "reject")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        let after = work_link(&infer("PAY-7"), &st, &OrgScope::All).unwrap();
        assert_eq!(
            after.work_suggested, None,
            "R9: a rejected pair is never proposed again"
        );
        assert_eq!(after.work, None);

        // A fresh inference, confirmed by the person, becomes the link.
        let row = work_link(&infer("PAY-8"), &st, &OrgScope::All).unwrap();
        let g8 = row.work_suggested.expect("suggested");
        let row = work_link(
            &WorkLinkArgs {
                link_id: Some(g8.link_id),
                ..link(sid, "confirm")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(row.work.and_then(|w| w.key).as_deref(), Some("PAY-8"));
    }

    /// Work graph M4.4: confirm / reject a suggestion by link id, and trust
    /// a project, through the one entry both transports share.
    #[test]
    fn suggestions_are_decided_by_link_id_and_projects_trusted() {
        let (st, sid) = store();
        let pid = {
            let s = st.lock().unwrap();
            s.create_local_work_item(Some("PAY-7"), "Retry").unwrap();
            detect::on_prompt(&s, sid, "see PAY-7 and PAY-8", false).unwrap();
            s.upsert_project("acme", "api", "/src/api").unwrap()
        };
        let sg = |st: &Mutex<Store>| {
            st.lock()
                .unwrap()
                .get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work_suggested
        };
        let first = sg(&st).expect("a suggestion");
        let row = work_link(
            &WorkLinkArgs {
                link_id: Some(first.link_id),
                ..link(sid, "confirm")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(row.work.unwrap().link_id, first.link_id);
        assert_eq!(
            work_link(&link(sid, "confirm"), &st, &OrgScope::All)
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
        let err = work_link(
            &WorkLinkArgs {
                link_id: Some(9999),
                ..link(sid, "reject")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);

        let t = trust_project(
            &WorkLinkArgs {
                action: "trust_project".into(),
                project_id: Some(pid),
                on: Some(true),
                ..Default::default()
            },
            &st,
        )
        .unwrap();
        assert_eq!(t.trusted, vec![pid]);
        let missing = trust_project(
            &WorkLinkArgs {
                action: "trust_project".into(),
                project_id: Some(pid + 50),
                on: Some(true),
                ..Default::default()
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(missing.code, codes::E_NOTFOUND);
    }

    /// Work graph M7.2: archive / unarchive / snooze / never through the one
    /// entry both transports share; dismiss has its own.
    #[test]
    fn lifecycle_actions_answer_the_row() {
        let (st, sid) = store();
        assert_eq!(
            work_link(&link(sid, "archive"), &st, &OrgScope::All)
                .unwrap_err()
                .code,
            codes::E_INVALID,
            "nothing to archive under"
        );
        work_link(
            &WorkLinkArgs {
                key: Some("abc-1".into()),
                ..link(sid, "link")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        let row = work_link(&link(sid, "archive"), &st, &OrgScope::All).unwrap();
        assert!(row.work.unwrap().archived_at.is_some());
        let row = work_link(&link(sid, "unarchive"), &st, &OrgScope::All).unwrap();
        assert_eq!(row.work.unwrap().archived_at, None);
        work_link(
            &WorkLinkArgs {
                days: Some(3),
                ..link(sid, "snooze")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap();
        let bad = WorkLinkArgs {
            days: Some(0),
            ..link(sid, "snooze")
        };
        assert_eq!(
            work_link(&bad, &st, &OrgScope::All).unwrap_err().code,
            codes::E_INVALID
        );
        work_link(&link(sid, "never"), &st, &OrgScope::All).unwrap();
        for own_entry in ["dismiss", "tidy_apply"] {
            assert_eq!(
                work_link(&link(sid, own_entry), &st, &OrgScope::All)
                    .unwrap_err()
                    .code,
                codes::E_INVALID
            );
        }
        let d = dismiss_reopened(
            &WorkLinkArgs {
                action: "dismiss".into(),
                item_id: Some(1),
                ..Default::default()
            },
            &st,
        );
        assert_eq!(d.unwrap_err().code, codes::E_NOTFOUND);
    }

    #[test]
    fn malformed_requests_are_refused() {
        let (st, sid) = store();
        for args in [
            link(sid, "link"),
            WorkLinkArgs {
                key: Some("A-1".into()),
                item_id: Some(1),
                ..link(sid, "reject")
            },
            link(sid, "unlink"),
            WorkLinkArgs {
                key: Some("A-1".into()),
                ..link(sid, "primary")
            },
            WorkLinkArgs {
                key: Some("A-1".into()),
                source: Some("branch".into()),
                ..link(sid, "link")
            },
        ] {
            let err = work_link(&args, &st, &OrgScope::All).unwrap_err();
            assert_eq!(err.code, codes::E_INVALID, "{args:?}");
        }
        assert!(
            work(
                &WorkArgs::default(),
                &st,
                &crate::service::view_scope::ViewScope::internal()
            )
            .unwrap()
            .is_empty(),
            "recent: none"
        );
        let both = WorkArgs {
            session_id: Some(sid),
            key: Some("A-1".into()),
            ..Default::default()
        };
        assert_eq!(
            work(
                &both,
                &st,
                &crate::service::view_scope::ViewScope::internal()
            )
            .unwrap_err()
            .code,
            codes::E_INVALID
        );
        let err = work_link(
            &WorkLinkArgs {
                key: Some("A-1".into()),
                ..link(sid + 99, "link")
            },
            &st,
            &OrgScope::All,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }
}
