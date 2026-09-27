//! The Work view's structure as it is read (work graph M14.1b): placement
//! rules and what a drafted one would move, saved views, and what moving a
//! local task to another org would change (`org_impact`, the preview of
//! M14.1c's `assign_org`). The writes are M14.1c's.
//!
//! Who reads what: a per-host token reads no rules and no views (a
//! session's agent does not reorganise the fleet) and no org impact; a
//! client bound to an org reads the rules that place a task it sees, its
//! own org's views, and no org impact (it may not move an org, D33);
//! everyone else all of it.

use super::view::{self, find_task, rule_matches, Graph, GroupRef};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::OrgScope;
use crate::store::{RuleConditions, Store, WorkRule, WorkView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// Longest group label, rule / view name.
pub const LABEL_MAX_CHARS: usize = 80;

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
        OrgScope::All => lock(store)?.work_rules(),
        OrgScope::Host { .. } => Ok(Vec::new()),
        OrgScope::Org { .. } => {
            let g = {
                let s = lock(store)?;
                Graph::load(&s)?
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
}

/// Most affected tasks one preview lists.
pub const PREVIEW_MAX: usize = 200;

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
        Graph::load(&s)?
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
    for t in &after {
        let Some(b) = before.iter().find(|x| x.task_id == t.task_id) else {
            continue;
        };
        let item = t.item_id.and_then(|i| g.items.get(&i));
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
    })
}

// --- saved views ----------------------------------------------------------------

/// Whose views a scope keeps: `None` every view (unrestricted), else the
/// views saved under that org. A host keeps none.
fn view_owner(scope: &OrgScope) -> Option<Option<i64>> {
    match scope {
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
        let row = l.session_id.and_then(|id| g.sessions.get(&id));
        let state = match (
            l.link.state.as_str(),
            l.link.ended_at.is_none() && row.is_some(),
        ) {
            ("confirmed", true) => "active",
            ("confirmed", false) => "ended",
            ("suggested", true) => "suggested",
            _ => continue,
        };
        let host = row
            .map(|r| r.host_alias.clone())
            .or_else(|| l.link.snap_host.clone());
        let session_org = match row {
            Some(r) => r.org_id,
            None => l.link.org_id,
        };
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
        if !scope.is_all() && !scope.sees_org(session_org) {
            continue;
        }
        links.push(ImpactLink {
            link_id: l.link.id,
            session_id: row.map(|r| r.id),
            name: row
                .map(|r| {
                    r.friendly_name
                        .clone()
                        .unwrap_or_else(|| r.tmux_name.clone())
                })
                .or_else(|| l.link.snap_name.clone())
                .or_else(|| l.link.snap_tmux.clone())
                .unwrap_or_else(|| "past session".into()),
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
    let sees = |viewer: Option<i64>, org: Option<i64>| org.is_none() || org == viewer;
    let mut hosts_losing = Vec::new();
    let mut hosts_gaining = Vec::new();
    for h in s.list_hosts()? {
        if !ran_on.contains(&h.alias) {
            continue;
        }
        match (sees(h.org_id, from), sees(h.org_id, to)) {
            (true, false) => hosts_losing.push(h.alias.clone()),
            (false, true) => hosts_gaining.push(h.alias.clone()),
            _ => {}
        }
    }
    let mut bound_clients_losing = 0;
    let mut bound_clients_gaining = 0;
    for c in s.active_client_tokens()? {
        let Some(o) = c.org_id else {
            continue;
        };
        match (sees(Some(o), from), sees(Some(o), to)) {
            (true, false) => bound_clients_losing += 1,
            (false, true) => bound_clients_gaining += 1,
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
        journal_entries,
        summaries,
        impact_token: token,
    })
}

/// `work { action: org_impact, task_id, org_id }` (`0`: no org): exactly
/// what moving a local task to another org changes, before it does.
pub fn org_impact(
    store: &Mutex<Store>,
    scope: &OrgScope,
    task_id: &str,
    org_id: Option<i64>,
) -> Result<OrgImpact, IpcError> {
    // Only a caller that may move an org reads what a move would change
    // (D33): the impact names every host and bound client of both orgs, and
    // the journal of sessions a scoped caller does not see.
    if !scope.is_all() {
        return Err(forbidden(
            "org_impact is not available to a per-host token or an org-bound client: it may \
             not move a task's org",
        ));
    }
    let to = org_arg(org_id)?;
    let s = lock(store)?;
    let g = Graph::load(&s)?;
    impact_of(&s, &g, scope, task_id, to)
}
