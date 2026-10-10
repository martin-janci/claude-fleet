//! The Work view's structure (work graph M14.1b reads, M14.1c writes):
//! where a task sits (placement), rules that place similar tasks, saved
//! views, and a local task's org — the one edit here that moves a boundary,
//! so it is previewed (`org_impact`) and applied only with the preview's
//! token (`assign_org`). Also the review inbox's batch of link decisions.
//!
//! Who reads what: a per-host token reads no rules and no views (a
//! session's agent does not reorganise the fleet) and no org impact; a
//! client bound to an org reads the rules that place a task it sees, its
//! own org's views, and no org impact (it may not move an org, D33);
//! everyone else all of it.
//!
//! Who writes what (the spec's *Mutations*): a per-host token none of it; a
//! client bound to an org places what it sees and keeps its own org's views
//! (D35), but writes no rule (D34: a rule reaches every org's tasks) and
//! moves no org (D33); everyone else all of it. Every write re-checks that
//! the task is visible, and answers an out-of-scope id as an unknown one.
//! Each write that replaces a row names the version it saw
//! (`expected_version`): another device's change meanwhile is `E_CONFLICT`
//! with the current value, never a silent overwrite.

use super::view::{
    self, check_filters, find_task, rule_matches, Graph, GroupRef, IdOrWord, WorkTask,
    WorkTreeFilters,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::OrgScope;
use crate::store::{Decider, RuleConditions, Store, WorkRule, WorkView};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// Longest group label, rule / view name.
pub const LABEL_MAX_CHARS: usize = 80;
/// Longest placement note.
pub const NOTE_MAX_CHARS: usize = 500;
/// Most decisions one `decide_batch` carries.
pub const BATCH_MAX: usize = 100;

/// A rule as a write names it.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct RuleInput {
    /// Absent: a new rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub conditions: RuleConditions,
    /// The group label.
    #[serde(default)]
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

/// A saved view as a write names it.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct ViewInput {
    /// Absent: a new view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub filters: WorkTreeFilters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

/// One decision of `decide_batch`.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct LinkDecision {
    pub session_id: i64,
    pub link_id: i64,
    /// confirm|reject|reconsider|ack
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
}

/// One decision's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionResult {
    pub link_id: i64,
    pub session_id: i64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The link's version after the decision (a successful one only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
}

/// `decide_batch`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchResult {
    pub results: Vec<DecisionResult>,
}

/// `{ deleted: true }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub deleted: bool,
}

fn forbidden(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_FORBIDDEN, msg)
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

/// A label, name or note: trimmed, not empty (unless `may_be_empty`), no
/// control character, at most `max` characters.
fn clean_text(
    raw: &str,
    what: &str,
    max: usize,
    may_be_empty: bool,
) -> Result<Option<String>, IpcError> {
    let t = raw.trim();
    if t.is_empty() {
        return if may_be_empty {
            Ok(None)
        } else {
            Err(invalid(format!("{what} must not be empty")))
        };
    }
    if t.chars().count() > max {
        return Err(invalid(format!("{what} is longer than {max} characters")));
    }
    if t.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(Some(t.to_string()))
}

fn no_host(scope: &OrgScope, what: &str) -> Result<(), IpcError> {
    if scope.host().is_some() {
        return Err(forbidden(format!(
            "{what} is not available to a per-host token: a session's agent does not reorganise \
             the fleet's work"
        )));
    }
    Ok(())
}

/// Refused to a per-host token and to a client bound to an org: the write
/// reaches beyond one org (a rule, an org move).
fn unbound_only(scope: &OrgScope, what: &str) -> Result<(), IpcError> {
    no_host(scope, what)?;
    if scope.bound_org().is_some() {
        return Err(forbidden(format!(
            "{what} is not available to a client bound to an org: it reaches beyond that org"
        )));
    }
    Ok(())
}

// --- placement ----------------------------------------------------------------

/// `work_link { action: place, task_id, group, note?, expected_version }`:
/// put a task in a group (empty `group` and no `note`: back to where it
/// would sit by itself). Navigation only, never a boundary; a
/// tracker-controlled value is never written back. `expected_version` is
/// the placement version the person saw (`0`: none). Answers the task as
/// it now reads.
///
/// **It takes the WHOLE scope** (multi-user M1, T9b), because what it ANSWERS
/// is a `WorkTask`, whose `sessions: Vec<TaskLink>` is every link of the task
/// — `{ session_id, name, host, branch, claude_status, needs_you, resumable }`
/// per link. The table's old exemption read "a task's group (`task_id`): the
/// work item, not a session", which is true of the write and false of the
/// answer; `Graph::load` is `OrgScope::All` for every paired client bound to
/// no org, and T8's result gate cannot net a `TaskLink` (it spells the host
/// `host` and carries no `tmux_name`). Same defect as `org_impact`'s, in the
/// sibling arm.
pub fn place(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    task_id: &str,
    group: Option<&str>,
    note: Option<&str>,
    expected_version: Option<i64>,
    by: &str,
) -> Result<WorkTask, IpcError> {
    let scope = &view.org;
    no_host(scope, "place")?;
    let expected = expected_version.ok_or_else(|| {
        invalid("place needs expected_version (0 when the task has no placement)")
    })?;
    let group = clean_text(group.unwrap_or_default(), "group", LABEL_MAX_CHARS, true)?;
    let note = clean_text(note.unwrap_or_default(), "note", NOTE_MAX_CHARS, true)?;
    let s = lock(store)?;
    let mut g = Graph::load_for(&s, view)?;
    // Visibility first: a task out of scope answers as an unknown one, and
    // its placement's version is never compared (no oracle).
    let (task, _) = find_task(&g, scope, task_id, false)?;
    let placement = s.set_work_placement(
        &task.task_id,
        group.as_deref(),
        note.as_deref(),
        expected,
        by,
    )?;
    // K5's follow-up: a person's placement confirms or corrects Jev's
    // proposed group. Never fails the placement.
    if let (Some(item_id), Some(label)) = (task.item_id, group.as_deref()) {
        if let Err(e) = crate::service::decide::work_placement::record_place(
            &s,
            item_id,
            label,
            crate::service::catalog::now_secs(),
        ) {
            tracing::warn!(
                "[decide] work_placement follow-up not recorded: {}",
                e.message
            );
        }
    }
    drop(s);
    // Only the placement changed: patch it into the graph already loaded
    // rather than reading every item, link and session again.
    match placement {
        Some(p) => g.placements.insert(task.task_id.clone(), p),
        None => g.placements.remove(&task.task_id),
    };
    Ok(find_task(&g, scope, &task.task_id, false)?.0)
}

// --- rules --------------------------------------------------------------------

fn clean_rule(r: &RuleInput) -> Result<(String, RuleConditions, String), IpcError> {
    let name = clean_text(&r.name, "rule name", LABEL_MAX_CHARS, false)?.unwrap_or_default();
    let group = clean_text(&r.group, "rule group", LABEL_MAX_CHARS, false)?.unwrap_or_default();
    let c = &r.conditions;
    let opt = |v: &Option<String>, what: &str| -> Result<Option<String>, IpcError> {
        match v {
            Some(x) => clean_text(x, what, LABEL_MAX_CHARS, true),
            None => Ok(None),
        }
    };
    let conditions = RuleConditions {
        tracker_id: c.tracker_id,
        container: opt(&c.container, "conditions.container")?,
        key_prefix: opt(&c.key_prefix, "conditions.key_prefix")?.map(|p| p.to_ascii_uppercase()),
        title_contains: opt(&c.title_contains, "conditions.title_contains")?,
        repo: opt(&c.repo, "conditions.repo")?,
    };
    if conditions.is_empty() {
        return Err(invalid(
            "a rule needs at least one condition (tracker_id, container, key_prefix, \
             title_contains or repo); a rule that places every task is not a rule",
        ));
    }
    Ok((name, conditions, group))
}

/// `work { action: rules }`. Rules are fleet-wide navigation, not work
/// data: a host reads none. A bound client reads only the rules that match
/// a task it sees and name no tracker it does not: a rule's conditions
/// (another org's tracker, its project keys, its repositories) are not its
/// to learn.
pub fn rules(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<WorkRule>, IpcError> {
    match scope {
        // This is the org boundary, not a privacy fence: a placement RULE is
        // a company's own configuration (its tracker, its project keys, its
        // repositories) and names no session.
        OrgScope::All => lock(store)?.work_rules(),
        OrgScope::Host { .. } => Ok(Vec::new()),
        OrgScope::Org { .. } => {
            let g = {
                let s = lock(store)?;
                Graph::load(&s, scope)?
            };
            let tasks = view::all_tasks(&g, scope, 0);
            Ok(g.rules
                .iter()
                .filter(|r| {
                    r.conditions.tracker_id.is_none_or(|t| {
                        g.trackers.get(&t).is_some_and(|t| scope.sees_org(t.org_id))
                    })
                })
                .filter(|r| {
                    tasks.iter().any(|t| {
                        let item = t.item_id.and_then(|i| g.items.get(&i));
                        rule_matches(r, item, t.key.as_deref(), &t.title, &t.repos)
                    })
                })
                .cloned()
                .collect())
        }
    }
}

/// One task a rule would move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleEffect {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    pub from: GroupRef,
    pub to: GroupRef,
}

/// `work { action: rule_preview, rule }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulePreview {
    /// Tasks the rule would move (at most 200 listed).
    pub affected: Vec<RuleEffect>,
    pub total: u32,
    /// Tasks it matches that a person placed: they stay where they are.
    pub kept_manual: u32,
    /// Open tasks (not done, not archived) the draft's conditions match now,
    /// whether or not it moves them and whether or not it is enabled: the
    /// editor's live "Matches 6 open tasks now" (gap plan G2.2). `0` from an
    /// older hub.
    #[serde(default)]
    pub matched: u32,
    /// The first [`MATCHED_SAMPLE`] of them, each its key, else its title.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_sample: Vec<String>,
}

/// Most affected tasks one preview lists.
pub const PREVIEW_MAX: usize = 200;
/// Most matched tasks one preview names.
pub const MATCHED_SAMPLE: usize = 3;

/// `work { action: rule_preview, rule }`: which of the caller's tasks the
/// rule — as drafted, in its place among the others — would move, before
/// anything is saved.
pub fn rule_preview(
    store: &Mutex<Store>,
    scope: &OrgScope,
    rule: &RuleInput,
) -> Result<RulePreview, IpcError> {
    no_host(scope, "rule_preview")?;
    let (name, conditions, group) = clean_rule(rule)?;
    let mut g = {
        let s = lock(store)?;
        Graph::load(&s, scope)?
    };
    let before = view::all_tasks(&g, scope, 0);
    let draft = WorkRule {
        id: rule.id.unwrap_or(i64::MAX),
        name,
        enabled: rule.enabled.unwrap_or(true),
        version: 0,
        conditions,
        group,
        created_at: 0,
        updated_at: 0,
    };
    match g.rules.iter_mut().find(|r| Some(r.id) == rule.id) {
        Some(r) => *r = draft.clone(),
        None => g.rules.push(draft.clone()),
    }
    g.rules.sort_by_key(|r| r.id);
    let after = view::all_tasks(&g, scope, 0);
    let mut affected = Vec::new();
    let mut kept_manual = 0;
    let mut matched = 0u32;
    let mut matched_sample = Vec::new();
    for t in &after {
        let Some(b) = before.iter().find(|x| x.task_id == t.task_id) else {
            continue;
        };
        let item = t.item_id.and_then(|i| g.items.get(&i));
        if !t.archived
            && t.stage != "done"
            && rule_matches(&draft, item, t.key.as_deref(), &t.title, &t.repos)
        {
            matched += 1;
            if matched_sample.len() < MATCHED_SAMPLE {
                matched_sample.push(t.key.clone().unwrap_or_else(|| t.title.clone()));
            }
        }
        if draft.enabled
            && t.group.source == "manual"
            && rule_matches(&draft, item, t.key.as_deref(), &t.title, &t.repos)
        {
            kept_manual += 1;
        }
        if b.group != t.group {
            affected.push(RuleEffect {
                task_id: t.task_id.clone(),
                key: t.key.clone(),
                title: t.title.clone(),
                from: b.group.clone(),
                to: t.group.clone(),
            });
        }
    }
    let total = affected.len() as u32;
    affected.truncate(PREVIEW_MAX);
    Ok(RulePreview {
        affected,
        total,
        kept_manual,
        matched,
        matched_sample,
    })
}

/// `work_link { action: rule_save, rule }`: create or replace a placement
/// rule (D34: placement only — a rule never links a session). What it
/// would move is `work { rule_preview }` of the same `rule`.
pub fn rule_save(
    store: &Mutex<Store>,
    scope: &OrgScope,
    rule: &RuleInput,
) -> Result<WorkRule, IpcError> {
    unbound_only(scope, "rule_save")?;
    let (name, conditions, group) = clean_rule(rule)?;
    let s = lock(store)?;
    if let Some(t) = conditions.tracker_id {
        if s.get_tracker(t)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("tracker {t} not found"),
            ));
        }
    }
    s.save_work_rule(
        rule.id,
        &name,
        rule.enabled.unwrap_or(true),
        &conditions,
        &group,
        rule.expected_version,
    )
}

/// `work_link { action: rule_delete, rule_id, expected_version? }`.
pub fn rule_delete(
    store: &Mutex<Store>,
    scope: &OrgScope,
    rule_id: i64,
    expected: Option<i64>,
) -> Result<Deleted, IpcError> {
    unbound_only(scope, "rule_delete")?;
    lock(store)?.delete_work_rule(rule_id, expected)?;
    Ok(Deleted { deleted: true })
}

// --- saved views ----------------------------------------------------------------

/// Whose views a scope keeps: `None` every view (unrestricted), else the
/// views saved under that org. A host keeps none.
fn view_owner(scope: &OrgScope) -> Option<Option<i64>> {
    match scope {
        // This is the org boundary, not a privacy fence: a saved VIEW is
        // stored under an org and is that org's configuration. It is a set
        // of filters, not any session's data.
        OrgScope::All => None,
        OrgScope::Org { org, .. } => Some(Some(*org)),
        OrgScope::Host { .. } => Some(Some(i64::MIN)),
    }
}

/// `work { action: views }`.
pub fn views(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<WorkView>, IpcError> {
    if scope.host().is_some() {
        return Ok(Vec::new());
    }
    lock(store)?.work_views(view_owner(scope))
}

/// A view's filters may name only an org or a tracker the caller sees: a
/// scoped caller naming another org's (or an unknown) id is answered as
/// the unknown id it is to it (work graph M14.1c).
fn check_view_filters(s: &Store, scope: &OrgScope, f: &WorkTreeFilters) -> Result<(), IpcError> {
    check_filters(f)?;
    // This is the org boundary, not a privacy fence: a filter may name only an org or tracker ID
    // this caller sees.
    if scope.is_all() {
        return Ok(());
    }
    if let Some(IdOrWord::Id(o)) = &f.org {
        if s.get_org(*o)?.is_none() || !scope.sees_org(Some(*o)) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("org {o} not found"),
            ));
        }
    }
    if let Some(IdOrWord::Id(t)) = &f.tracker {
        if !s
            .get_tracker(*t)?
            .is_some_and(|tr| scope.sees_org(tr.org_id))
        {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("tracker {t} not found"),
            ));
        }
    }
    Ok(())
}

/// `work_link { action: view_save, view }`: shared on the hub (D35); a
/// bound client's views are its org's, and it can neither see nor replace
/// another's.
pub fn view_save(
    store: &Mutex<Store>,
    scope: &OrgScope,
    v: &ViewInput,
) -> Result<WorkView, IpcError> {
    no_host(scope, "view_save")?;
    let name = clean_text(&v.name, "view name", LABEL_MAX_CHARS, false)?.unwrap_or_default();
    let s = lock(store)?;
    check_view_filters(&s, scope, &v.filters)?;
    let filters = serde_json::to_value(&v.filters)
        .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
    s.save_work_view(
        v.id,
        &name,
        &filters,
        scope.bound_org().map(Some),
        v.expected_version,
    )
}

/// `work_link { action: view_delete, view_id, expected_version? }`.
pub fn view_delete(
    store: &Mutex<Store>,
    scope: &OrgScope,
    view_id: i64,
    expected: Option<i64>,
) -> Result<Deleted, IpcError> {
    no_host(scope, "view_delete")?;
    lock(store)?.delete_work_view(view_id, view_owner(scope), expected)?;
    Ok(Deleted { deleted: true })
}

// --- the org of a local task ------------------------------------------------------

/// One link an org move affects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactLink {
    pub link_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// active | ended | suggested
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_org: Option<i64>,
    /// After the move the task's org and this session's differ: the link
    /// stays, flagged cross-org (and listed for review).
    pub becomes_cross_org: bool,
}

/// `work { action: org_impact }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgImpact {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_org: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_org: Option<i64>,
    pub allowed: bool,
    /// tracker_controlled | bare_key | same_org, when not allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub links: Vec<ImpactLink>,
    /// Hosts that ran this work and would stop seeing it / start seeing it.
    pub hosts_losing: Vec<String>,
    pub hosts_gaining: Vec<String>,
    /// Paired clients bound to an org that would stop / start seeing it.
    pub bound_clients_losing: u32,
    pub bound_clients_gaining: u32,
    /// The people whose org-bound devices are among those: who loses /
    /// gains access to the task by the move (gap plan G2.2's "Ondrej loses
    /// access"), by display name, sorted. A device that belongs to someone
    /// who keeps access on another of theirs still counts them: the line
    /// names what the device sees. Empty from an older hub.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people_losing: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people_gaining: Vec<String>,
    /// Journal rows (conversations, notes, handovers) and summaries of
    /// this work whose readers change with it.
    pub journal_entries: u32,
    pub summaries: u32,
    /// Pass it to `assign_org`: the move is applied only while the impact
    /// is still this one.
    pub impact_token: String,
}

fn org_arg(org_id: Option<i64>) -> Result<Option<i64>, IpcError> {
    match org_id {
        None => Err(invalid("this action needs org_id (0 for no org)")),
        Some(0) => Ok(None),
        Some(n) if n > 0 => Ok(Some(n)),
        Some(n) => Err(invalid(format!("org_id {n} is not an org id"))),
    }
}

fn impact_of(
    s: &Store,
    g: &Graph,
    scope: &OrgScope,
    task_id: &str,
    to: Option<i64>,
) -> Result<OrgImpact, IpcError> {
    let (task, _) = find_task(g, scope, task_id, false)?;
    if let Some(o) = to {
        if s.get_org(o)?.is_none() || !scope.sees_org(Some(o)) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("org {o} not found"),
            ));
        }
    }
    let item = task.item_id.and_then(|i| g.items.get(&i));
    let from = item.and_then(|i| g.item_org(i));
    let (allowed, reason) = match (task.kind.as_str(), item) {
        ("tracker", _) => (false, Some("tracker_controlled")),
        ("ref", _) | (_, None) => (false, Some("bare_key")),
        _ if from == to => (false, Some("same_org")),
        _ => (true, None),
    };
    let item_id = task.item_id;
    let mut links = Vec::new();
    let mut ran_on: BTreeSet<String> = BTreeSet::new();
    let mut conversations: Vec<String> = Vec::new();
    for l in g
        .links
        .iter()
        .filter(|l| item_id.is_some() && l.link.item_id == item_id)
    {
        // The PERSON fence, before anything is read off the link (multi-user
        // M1, T8d; its ENDED half T9b): a link whose session this reader may
        // not see is not listed, its host is not added to `ran_on`, and its
        // conversations are not counted. It must NOT fall through to the
        // snapshot arms below, which would name the session by `snap_name`
        // and its machine by `snap_host` — exactly what the fence withholds.
        // `Graph::link_hidden` is the one clause, shared with
        // `Graph::link_visible`, applied here because `impact_of` projects
        // the links itself rather than going through it.
        if g.link_hidden(l) {
            continue;
        }
        // The view's own state (a rejection moves no one), name and org.
        let Some(state) = g.state_of(l).filter(|s| *s != "rejected") else {
            continue;
        };
        let row = l.session_id.and_then(|id| g.sessions.get(&id));
        let host = row
            .map(|r| r.host_alias.clone())
            .or_else(|| l.link.snap_host.clone());
        let session_org = g.session_org(l);
        if let Some(h) = &host {
            ran_on.insert(h.clone());
        }
        if let Some(ids) = l
            .link
            .snap_claude_ids
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
        {
            conversations.extend(ids);
        }
        if let Some(c) = row.and_then(|r| r.claude_session_id.clone()) {
            conversations.push(c);
        }
        // What the caller may not see of the link is not listed (an
        // unrestricted caller — the only one that may move an org — sees
        // all of it).
        // This is the org boundary, not a privacy fence: which orgs' links a move's preview
        // lists. `Graph::load_for` has already emptied the person-invisible rows out of the
        // graph, and `Graph::link_hidden` keeps their snapshots out.
        if !scope.is_all() && !scope.sees_org(session_org) {
            continue;
        }
        links.push(ImpactLink {
            link_id: l.link.id,
            session_id: row.map(|r| r.id),
            name: view::link_name(row, l),
            host,
            state: state.into(),
            session_org,
            becomes_cross_org: matches!((to, session_org), (Some(a), Some(b)) if a != b),
        });
    }
    conversations.sort();
    conversations.dedup();
    let rows = s.journal_for_conversations(&conversations)?;
    let journal_entries = rows.len() as u32;
    let summaries = rows.iter().filter(|r| r.kind == "summary").count() as u32;
    // A host sees its own org's and unassigned work (`OrgScope::Host`).
    let host_sees = |viewer: Option<i64>, org: Option<i64>| org.is_none() || org == viewer;
    let mut hosts_losing = Vec::new();
    let mut hosts_gaining = Vec::new();
    for h in s.list_hosts()? {
        if !ran_on.contains(&h.alias) {
            continue;
        }
        match (host_sees(h.org_id, from), host_sees(h.org_id, to)) {
            (true, false) => hosts_losing.push(h.alias.clone()),
            (false, true) => hosts_gaining.push(h.alias.clone()),
            _ => {}
        }
    }
    let mut bound_clients_losing = 0;
    let mut bound_clients_gaining = 0;
    // A bound client sees what its `OrgScope::Org` does: unassigned work
    // only while its org's `bound_sees_unassigned` is on (D31). One scope
    // per org, read once.
    let mut client_scopes: HashMap<i64, OrgScope> = HashMap::new();
    let mut people_losing: BTreeSet<String> = BTreeSet::new();
    let mut people_gaining: BTreeSet<String> = BTreeSet::new();
    let person_name = |id: Option<i64>| -> Result<Option<String>, IpcError> {
        Ok(match id {
            Some(p) => s.get_person(p)?.map(|p| p.display_name.unwrap_or(p.name)),
            None => None,
        })
    };
    for c in s.active_client_tokens()? {
        let Some(o) = c.org_id else {
            continue;
        };
        let cs = match client_scopes.entry(o) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(OrgScope::for_client(s, o)?),
        };
        match (cs.sees_org(from), cs.sees_org(to)) {
            (true, false) => {
                bound_clients_losing += 1;
                people_losing.extend(person_name(c.person_id)?);
            }
            (false, true) => {
                bound_clients_gaining += 1;
                people_gaining.extend(person_name(c.person_id)?);
            }
            _ => {}
        }
    }
    let token = {
        use sha2::Digest;
        let basis = serde_json::json!({
            "task": task.task_id, "from": from, "to": to,
            "links": links.iter().map(|l| (l.link_id, l.session_org)).collect::<Vec<_>>(),
            "losing": hosts_losing, "gaining": hosts_gaining,
            "clients": [bound_clients_losing, bound_clients_gaining],
        });
        hex::encode(&sha2::Sha256::digest(basis.to_string().as_bytes())[..12])
    };
    Ok(OrgImpact {
        task_id: task.task_id,
        from_org: from,
        to_org: to,
        allowed,
        reason: reason.map(str::to_string),
        links,
        hosts_losing,
        hosts_gaining,
        bound_clients_losing,
        bound_clients_gaining,
        people_losing: people_losing.into_iter().collect(),
        people_gaining: people_gaining.into_iter().collect(),
        journal_entries,
        summaries,
        impact_token: token,
    })
}

/// `work { action: org_impact, task_id, org_id }` (`0`: no org): exactly
/// what moving a local task to another org changes, before it does.
/// Multi-user M1 (T8d): it takes the WHOLE scope. `is_all()` below is the
/// AUTHORITY check it was always meant to be — only a caller that may move an
/// org may preview the move — and is not, and never was, a privacy fence:
/// `OrgScope::All` is what the master and every paired client bound to no org
/// alike resolve to, so on a two-person hub the old `Graph::load` handed an
/// ordinary person's laptop an `ImpactLink { session_id, name, host }` for
/// every session on the task, whoever owned it. `Graph::load_for` empties the
/// invisible rows out of the graph first, and `impact_of`'s own
/// `Graph::link_hidden` skip keeps their snapshots out too.
pub fn org_impact(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    task_id: &str,
    org_id: Option<i64>,
) -> Result<OrgImpact, IpcError> {
    let scope = &view.org;
    // Only a caller that may move an org reads what a move would change
    // (D33): the impact names every host and bound client of both orgs.
    // This is the org boundary, not a privacy fence: it is the AUTHORITY to move an org (D33),
    // which the doc above has said since T8d.
    if !scope.is_all() {
        return Err(forbidden(
            "org_impact is not available to a per-host token or an org-bound client: it may \
             not move a task's org",
        ));
    }
    let to = org_arg(org_id)?;
    let s = lock(store)?;
    let g = Graph::load_for(&s, view)?;
    impact_of(&s, &g, scope, task_id, to)
}

/// `work_link { action: assign_org, task_id, org_id, impact_token }`: move
/// a local task to another org (or to none, `0`), if the impact is still
/// the one the person previewed (`E_CONFLICT` with the fresh impact
/// otherwise). D33: the master or a full unbound client; never a per-host
/// token nor a bound client. The links stay; each is judged by every read
/// from now on (a link's org is its item's), and the live sessions' rows
/// are re-announced, so a reader outside the new org stops receiving the
/// task from its next read or frame.
pub fn assign_org(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    task_id: &str,
    org_id: Option<i64>,
    impact_token: Option<&str>,
) -> Result<WorkTask, IpcError> {
    let scope = &view.org;
    unbound_only(scope, "assign_org")?;
    let to = org_arg(org_id)?;
    let token = impact_token
        .ok_or_else(|| invalid("assign_org needs the impact_token of a fresh org_impact"))?;
    let s = lock(store)?;
    // The same load as `org_impact`, and that is load-bearing rather than
    // tidiness: `impact_token` is a hash OVER `impact.links`, so a move
    // previewed through the person-fenced graph can only be confirmed
    // against the same one (T8d).
    let mut g = Graph::load_for(&s, view)?;
    let impact = impact_of(&s, &g, scope, task_id, to)?;
    if !impact.allowed {
        return Err(match impact.reason.as_deref() {
            Some("tracker_controlled") => forbidden(
                "this task's org is its tracker's; move the tracker instead \
                 (work_admin assign_tracker, master token)",
            ),
            Some("same_org") => invalid("the task is already in that org"),
            _ => invalid("a bare key has no item to carry an org; name the work first"),
        });
    }
    if impact.impact_token != token {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            "the impact of this move changed since it was previewed; review it again",
        )
        .with_details(serde_json::to_value(&impact).unwrap_or_default()));
    }
    let item_id = impact
        .task_id
        .strip_prefix("item:")
        .and_then(|n| n.parse::<i64>().ok())
        .ok_or_else(|| invalid("not a local item"))?;
    s.set_local_item_org(item_id, to)?;
    drop(s);
    // Only the item's own org changed (a link's org is its item's, read
    // from the graph): patch it in rather than loading the graph again.
    if let Some(item) = g.items.get_mut(&item_id) {
        item.own_org = to;
    }
    Ok(find_task(&g, scope, &impact.task_id, false)?.0)
}

// --- batch decisions ----------------------------------------------------------------

/// `work_link { action: decide_batch, decisions }`: each decision on its
/// own — its session through `gate` (the transport's host / bound-client
/// session fence), then exactly the single action's checks (scope, version,
/// cross-org). One failing never undoes or stops the others; the answer
/// says, per item and in order, which did what. There is no
/// `force_cross_org` in a batch: a cross-org confirm is decided alone.
/// `decider` is the caller's, exactly as for a single decision (D34).
pub fn decide_batch(
    store: &Mutex<Store>,
    scope: &OrgScope,
    decider: Decider,
    decisions: &[LinkDecision],
    gate: &dyn Fn(i64) -> Result<(), IpcError>,
) -> Result<BatchResult, IpcError> {
    if decisions.is_empty() {
        return Err(invalid("decide_batch needs decisions"));
    }
    if decisions.len() > BATCH_MAX {
        return Err(invalid(format!(
            "decide_batch takes at most {BATCH_MAX} decisions"
        )));
    }
    let mut results = Vec::with_capacity(decisions.len());
    for d in decisions {
        let outcome = (|| -> Result<(), IpcError> {
            if !matches!(
                d.decision.as_str(),
                "confirm" | "reject" | "reconsider" | "ack"
            ) {
                return Err(invalid(format!(
                    "decision is confirm, reject, reconsider or ack, not {:?}",
                    d.decision
                )));
            }
            gate(d.session_id)?;
            let args = super::WorkLinkArgs {
                session_id: Some(d.session_id),
                action: d.decision.clone(),
                link_id: Some(d.link_id),
                expected_version: d.expected_version,
                primary: d.primary,
                ..Default::default()
            };
            super::work_link_as(&args, store, scope, decider).map(|_| ())
        })();
        results.push(match outcome {
            Ok(()) => DecisionResult {
                link_id: d.link_id,
                session_id: d.session_id,
                ok: true,
                code: None,
                message: None,
                version: lock(store)?.work_link_version(d.link_id)?,
            },
            Err(e) => DecisionResult {
                link_id: d.link_id,
                session_id: d.session_id,
                ok: false,
                code: Some(e.code.to_string()),
                message: Some(e.message.clone()),
                version: None,
            },
        });
    }
    Ok(BatchResult { results })
}
