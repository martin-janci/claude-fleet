//! Organisations (work graph M5): the scope every work read and write runs
//! under, the zero-config scope list, suggestions, and the admin actions.
//!
//! **One scope, two strengths** (plan, decision 1). [`OrgScope::All`] is the
//! master, a paired client and the desktop: every org, and the org is only a
//! view filter (the UI's selector). [`OrgScope::Host`] is a per-host token —
//! every in-session Claude — and there the org is a BOUNDARY:
//!
//! * work data (items, links, journal and handover text, trackers,
//!   `SessionRow.work` / `work_suggested` / `work_rejected`) is visible only
//!   when its org is the host's or unassigned; a host with no org sees
//!   unassigned work only ([`OrgScope::sees_org`]);
//! * sessions are fenced too, but only between orgs that turned
//!   `isolate_sessions` on (decision D7, default off) — [`OrgScope::sees_session`].
//!
//! The scope is computed in exactly one place, `Caller::org_scope` (MCP), and
//! is `All` for every Tauri command (the desktop is the master; a paired
//! desktop reaches the hub as a client). Service functions take it and filter
//! with the predicates here; nothing at a call site decides visibility.
//!
//! **No existence oracle.** Something out of scope named by id answers exactly
//! as an id that does not exist ([`not_found`]); by key or URL, exactly as a
//! key nothing is linked to on the host ([`not_visible_key`]).

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{OrgRow, OrgRuleRow, SessionRow, Store, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// Who is asking, for every work read and write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgScope {
    /// Master, a paired client, the desktop: everything.
    All,
    /// A per-host token.
    Host {
        alias: String,
        /// The host's org; `None` sees unassigned data only.
        org: Option<i64>,
        /// Orgs with `isolate_sessions` on (D7).
        isolated: BTreeSet<i64>,
    },
}

impl OrgScope {
    /// The scope of a per-host token for `alias`, read now.
    pub fn for_host(s: &Store, alias: &str) -> Result<Self, IpcError> {
        Ok(OrgScope::Host {
            alias: alias.to_string(),
            org: s.host_org(alias)?,
            isolated: s.isolated_orgs()?,
        })
    }

    pub fn is_all(&self) -> bool {
        matches!(self, OrgScope::All)
    }

    /// The host a per-host token is bound to.
    pub fn host(&self) -> Option<&str> {
        match self {
            OrgScope::All => None,
            OrgScope::Host { alias, .. } => Some(alias),
        }
    }

    /// Work data of `org` is visible: always for `All`; for a host, its own
    /// org's and unassigned data.
    pub fn sees_org(&self, org: Option<i64>) -> bool {
        match self {
            OrgScope::All => true,
            OrgScope::Host { org: mine, .. } => org.is_none() || org == *mine,
        }
    }

    /// A link (its `org_id` resolved by `Store::fill_link_orgs`).
    pub fn sees_link(&self, l: &WorkLinkRow) -> bool {
        self.sees_org(l.org_id)
    }

    /// Session isolation (D7). A host always sees its own host's sessions
    /// and unassigned ones; another org's session is hidden when either org
    /// isolates sessions.
    pub fn sees_session(&self, row_host: &str, row_org: Option<i64>) -> bool {
        match self {
            OrgScope::All => true,
            OrgScope::Host {
                alias,
                org,
                isolated,
            } => {
                if row_host == alias || row_org.is_none() || row_org == *org {
                    return true;
                }
                let theirs = row_org.is_some_and(|o| isolated.contains(&o));
                let mine = org.is_some_and(|o| isolated.contains(&o));
                !(theirs || mine)
            }
        }
    }

    /// [`Self::sees_session`] over a row.
    pub fn sees_row(&self, row: &SessionRow) -> bool {
        self.sees_session(&row.host_alias, row.org_id)
    }

    /// Take out of a session row the work data this scope may not read: all
    /// of it for a session outside the scope's orgs, else a link (primary or
    /// suggestion) whose own org is outside.
    pub fn redact_row(&self, row: &mut SessionRow) {
        if self.is_all() {
            return;
        }
        if !self.sees_org(row.org_id) {
            row.work = None;
            row.work_suggested = None;
            row.work_rejected.clear();
            return;
        }
        if row.work.as_ref().is_some_and(|w| !self.sees_org(w.org_id)) {
            row.work = None;
        }
        if row
            .work_suggested
            .as_ref()
            .is_some_and(|w| !self.sees_org(w.org_id))
        {
            row.work_suggested = None;
        }
    }

    /// [`Self::redact_row`] over serialised output: every JSON object that
    /// is a session row (it has `tmux_name` and a work field) anywhere in
    /// `v`. `session_org` answers a row object's org — the MCP gate looks it
    /// up by `id` (a projection may have dropped `org_id`); an event frame
    /// carries the whole row, so its own `org_id` is read.
    pub fn redact_json(
        &self,
        v: &mut serde_json::Value,
        session_org: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Option<i64>,
    ) {
        if self.is_all() {
            return;
        }
        match v {
            serde_json::Value::Array(items) => {
                for i in items {
                    self.redact_json(i, session_org);
                }
            }
            serde_json::Value::Object(map) => {
                let is_row = map.contains_key("tmux_name")
                    && WORK_FIELDS.iter().any(|k| map.contains_key(*k));
                if is_row {
                    let org = session_org(map);
                    if !self.sees_org(org) {
                        for k in WORK_FIELDS {
                            map.remove(*k);
                        }
                    } else {
                        for k in ["work", "work_suggested"] {
                            let link_org = map
                                .get(k)
                                .and_then(|w| w.get("org_id"))
                                .and_then(serde_json::Value::as_i64)
                                // A link without its own org is the session's.
                                .or(org);
                            let present = map.get(k).is_some_and(|w| !w.is_null());
                            if present && !self.sees_org(link_org) {
                                map.remove(k);
                            }
                        }
                    }
                }
                for (_, child) in map.iter_mut() {
                    self.redact_json(child, session_org);
                }
            }
            _ => {}
        }
    }
}

/// The `SessionRow` fields that are work data.
pub const WORK_FIELDS: &[&str] = &["work", "work_suggested", "work_rejected"];

/// What an id outside the scope answers: the words an unknown id gets.
pub fn not_found(what: &str, id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("{what} {id} not found"))
}

/// What a key or URL outside a host's scope answers, whether or not
/// anything by that name exists.
pub fn not_visible_key(host: &str, key: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{key} is not visible to host {host}: a per-host token reads only the work its \
             own host's sessions do, within the host's organisation"
        ),
    )
}

/// One entry of `work { action: scopes }`: a named org, or — zero-config —
/// a project owner no org covers, or the unassigned rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeEntry {
    /// A named org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// An owner-derived pseudo-scope (no org covers the owner's sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The sessions nothing places: no org, no owner.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unassigned: bool,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub session_count: usize,
    /// Sessions waiting on a person (`service::attention::needs_attention`).
    pub needs_you: usize,
}

/// `work { action: scopes }`: the named orgs (each with its live sessions),
/// then an owner pseudo-scope per project owner whose sessions no org
/// covers, then the unassigned rest when there is any. A host-bound caller
/// sees only its org and unassigned work.
pub fn scopes(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<ScopeEntry>, IpcError> {
    let s = lock(store)?;
    let orgs = s.list_orgs()?;
    let projects: BTreeMap<i64, String> = s
        .list_projects()?
        .into_iter()
        .filter(|p| p.owner != "local" && !p.system)
        .map(|p| (p.id, p.owner))
        .collect();
    let rows: Vec<SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| r.status != "ghost" && scope.sees_row(r) && scope.sees_org(r.org_id))
        .collect();
    let needs = |r: &SessionRow| crate::service::attention::needs_attention(r).is_some();
    let mut out = Vec::new();
    for o in orgs.iter().filter(|o| scope.sees_org(Some(o.id))) {
        let mine: Vec<&SessionRow> = rows.iter().filter(|r| r.org_id == Some(o.id)).collect();
        out.push(ScopeEntry {
            id: Some(o.id),
            owner: None,
            unassigned: false,
            label: o.name.clone(),
            color: o.color.clone(),
            session_count: mine.len(),
            needs_you: mine.iter().filter(|r| needs(r)).count(),
        });
    }
    let mut by_owner: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut rest = (0usize, 0usize);
    for r in rows.iter().filter(|r| r.org_id.is_none()) {
        let slot = match r.project_id.and_then(|p| projects.get(&p)) {
            Some(owner) => by_owner.entry(owner.clone()).or_default(),
            None => &mut rest,
        };
        slot.0 += 1;
        slot.1 += usize::from(needs(r));
    }
    for (owner, (n, nu)) in by_owner {
        out.push(ScopeEntry {
            id: None,
            label: owner.clone(),
            owner: Some(owner),
            unassigned: false,
            color: None,
            session_count: n,
            needs_you: nu,
        });
    }
    if rest.0 > 0 {
        out.push(ScopeEntry {
            id: None,
            owner: None,
            unassigned: true,
            label: "Unassigned".into(),
            color: None,
            session_count: rest.0,
            needs_you: rest.1,
        });
    }
    Ok(out)
}

/// `work { action: orgs }`: the orgs with their rules, hosts and trackers,
/// for Settings → Organisations (read-only; changes are `work_admin`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgDetail {
    #[serde(flatten)]
    pub org: OrgRow,
    #[serde(default)]
    pub rules: Vec<OrgRuleRow>,
    #[serde(default)]
    pub hosts: Vec<String>,
    /// `(id, name)` of the trackers assigned to it.
    #[serde(default)]
    pub trackers: Vec<OrgTrackerRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgTrackerRef {
    pub id: i64,
    pub name: String,
}

pub fn org_details(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<OrgDetail>, IpcError> {
    org_details_locked(&*lock(store)?, scope)
}

// --- administration (work graph M5.2) ------------------------------------------

/// The org actions of `work_admin` — Master-only on the MCP surface
/// (`work_admin` is `Access::Master`), `LocalOnly` on a paired desktop, and
/// `fleet-hub org …` on the hub. A host can never move itself: no host-bound
/// path reaches here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgAction {
    ListOrgs,
    AddOrg,
    UpdateOrg,
    RemoveOrg,
    AddRule,
    RemoveRule,
    AssignHost,
    UnassignHost,
    AssignTracker,
}

impl OrgAction {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "list_orgs" => OrgAction::ListOrgs,
            "add_org" => OrgAction::AddOrg,
            "update_org" => OrgAction::UpdateOrg,
            "remove_org" => OrgAction::RemoveOrg,
            "add_rule" | "add_org_rule" => OrgAction::AddRule,
            "remove_rule" | "remove_org_rule" => OrgAction::RemoveRule,
            "assign_host" => OrgAction::AssignHost,
            "unassign_host" => OrgAction::UnassignHost,
            "assign_tracker" => OrgAction::AssignTracker,
            _ => return None,
        })
    }
}

fn need<T: Clone>(v: &Option<T>, action: &str, field: &str) -> Result<T, IpcError> {
    v.clone()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{action} needs {field}")))
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

/// Run one org action (the caller already passed the Master gate and, for a
/// removal, the confirmation gate). Every change that can move a session
/// between orgs re-announces the moved sessions (`Store::announce_org_moves`).
pub fn admin(
    action: OrgAction,
    args: &crate::service::trackers::admin::WorkAdminArgs,
    s: &Store,
) -> Result<serde_json::Value, IpcError> {
    let name = args.action.as_str();
    let before = s.session_orgs()?;
    let out = match action {
        OrgAction::ListOrgs => {
            return to_json(&org_details_locked(s, &OrgScope::All)?);
        }
        OrgAction::AddOrg => to_json(&s.add_org(
            &need(&args.name, name, "name")?,
            args.color.as_deref(),
            args.isolate_sessions.unwrap_or(false),
        )?)?,
        OrgAction::UpdateOrg => to_json(&s.update_org(
            need(&args.org_id, name, "org_id")?,
            args.name.as_deref(),
            args.color.as_deref(),
            args.isolate_sessions,
        )?)?,
        OrgAction::RemoveOrg => {
            let id = need(&args.org_id, name, "org_id")?;
            let org = s.get_org(id)?.ok_or_else(|| not_found("org", id))?;
            let trackers = s.trackers_of_org(id)?;
            if !trackers.is_empty() {
                let names: Vec<String> = trackers
                    .iter()
                    .map(|(tid, n)| format!("{n} (tracker {tid})"))
                    .collect();
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "org {:?} still owns {}; move them first (assign_tracker)",
                        org.name,
                        names.join(", ")
                    ),
                )
                .with_details(serde_json::json!({
                    "trackers": trackers.iter().map(|(i, _)| i).collect::<Vec<_>>()
                })));
            }
            s.remove_org(id)?;
            serde_json::json!({ "removed": id })
        }
        OrgAction::AddRule => to_json(&s.add_org_rule(OrgRuleRow {
            id: 0,
            org_id: need(&args.org_id, name, "org_id")?,
            owner: args.owner.clone(),
            repo: args.repo.clone(),
            path_prefix: args.path_prefix.clone(),
            host_alias: args.host_alias.clone(),
        })?)?,
        OrgAction::RemoveRule => {
            let id = need(&args.rule_id, name, "rule_id")?;
            if !s.remove_org_rule(id)? {
                return Err(not_found("rule", id));
            }
            serde_json::json!({ "removed": id })
        }
        OrgAction::AssignHost => {
            let host = need(&args.host_alias, name, "host_alias")?;
            let org = need(&args.org_id, name, "org_id")?;
            s.set_host_org(&host, Some(org))?;
            serde_json::json!({ "host_alias": host, "org_id": org })
        }
        OrgAction::UnassignHost => {
            let host = need(&args.host_alias, name, "host_alias")?;
            s.set_host_org(&host, None)?;
            serde_json::json!({ "host_alias": host, "org_id": null })
        }
        OrgAction::AssignTracker => {
            let id = need(&args.tracker_id, name, "tracker_id")?;
            s.set_tracker_org(id, args.org_id)?;
            s.emit_tracker(id)?;
            to_json(&s.require_tracker(id)?)?
        }
    };
    s.announce_org_moves(&before)?;
    Ok(out)
}

fn org_details_locked(s: &Store, scope: &OrgScope) -> Result<Vec<OrgDetail>, IpcError> {
    let rules = s.list_org_rules()?;
    let hosts = s.list_hosts()?;
    let mut out = Vec::new();
    for o in s.list_orgs()? {
        if !scope.sees_org(Some(o.id)) {
            continue;
        }
        out.push(OrgDetail {
            rules: rules.iter().filter(|r| r.org_id == o.id).cloned().collect(),
            hosts: hosts
                .iter()
                .filter(|h| h.org_id == Some(o.id))
                .map(|h| h.alias.clone())
                .collect(),
            trackers: s
                .trackers_of_org(o.id)?
                .into_iter()
                .map(|(id, name)| OrgTrackerRef { id, name })
                .collect(),
            org: o,
        });
    }
    Ok(out)
}

/// One proposal of `work { action: org_suggestions }`: create org `name`
/// from `owner/*` and/or for a tracker. Never applied automatically; the UI
/// offers it as one click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgSuggestion {
    pub name: String,
    /// Add the rule `owner/*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Assign this tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// Live sessions the rule would place.
    pub sessions: usize,
    /// Why, in one line.
    pub reason: String,
}

/// The first label of an Atlassian site (`https://acme.atlassian.net` →
/// `acme`), which is usually the company.
fn site_label(site_url: &str) -> Option<String> {
    let host = site_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()?;
    let first = host.split('.').next()?;
    (!first.is_empty()).then(|| first.to_ascii_lowercase())
}

/// Proposals, from what fleet already sees:
///
/// * an owner of live sessions that no rule names and that no org covers
///   → "Create org <owner> from `<owner>/*`";
/// * a tracker without an org → an org named after its site, with the
///   owner of the same name when there is one (`acme.atlassian.net` and
///   GitHub `acme` are one company more often than not).
///
/// Empty for a per-host token: it cannot act on any of it.
pub fn org_suggestions(
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<Vec<OrgSuggestion>, IpcError> {
    if !scope.is_all() {
        return Ok(Vec::new());
    }
    let s = lock(store)?;
    let owners: BTreeMap<i64, String> = s
        .list_projects()?
        .into_iter()
        .filter(|p| p.owner != "local" && !p.system)
        .map(|p| (p.id, p.owner))
        .collect();
    let named: BTreeSet<String> = s
        .list_org_rules()?
        .into_iter()
        .filter_map(|r| r.owner.map(|o| o.to_ascii_lowercase()))
        .collect();
    let org_names: BTreeSet<String> = s
        .list_orgs()?
        .into_iter()
        .map(|o| o.name.to_ascii_lowercase())
        .collect();
    // Uncovered owners of live, unassigned sessions, with their counts.
    let mut uncovered: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for r in s.list_all_sessions()? {
        if r.status == "ghost" || r.org_id.is_some() {
            continue;
        }
        let Some(owner) = r.project_id.and_then(|p| owners.get(&p)) else {
            continue;
        };
        if named.contains(&owner.to_ascii_lowercase()) {
            continue;
        }
        uncovered
            .entry(owner.to_ascii_lowercase())
            .or_insert_with(|| (owner.clone(), 0))
            .1 += 1;
    }
    let mut out = Vec::new();
    let mut used_owner = BTreeSet::new();
    for t in s
        .list_trackers()?
        .into_iter()
        .filter(|t| t.org_id.is_none())
    {
        let Some(label) = site_label(&t.site_url) else {
            continue;
        };
        if org_names.contains(&label) {
            continue;
        }
        let owner = uncovered.get(&label).cloned();
        if owner.is_some() {
            used_owner.insert(label.clone());
        }
        out.push(OrgSuggestion {
            name: owner.as_ref().map(|o| o.0.clone()).unwrap_or(label.clone()),
            reason: match &owner {
                Some((o, _)) => format!("tracker {} and GitHub owner {o} share a name", t.name),
                None => format!("tracker {} has no org yet", t.name),
            },
            sessions: owner.as_ref().map(|o| o.1).unwrap_or(0),
            owner: owner.map(|o| o.0),
            tracker_id: Some(t.id),
        });
    }
    for (key, (owner, n)) in uncovered {
        if used_owner.contains(&key) || org_names.contains(&key) {
            continue;
        }
        out.push(OrgSuggestion {
            name: owner.clone(),
            reason: format!(
                "{n} live session{} under {owner}/*",
                if n == 1 { "" } else { "s" }
            ),
            owner: Some(owner),
            tracker_id: None,
            sessions: n,
        });
    }
    // Only worth offering when it would separate something: two or more
    // scopes after it, or a tracker to attach.
    if out.len() < 2 && out.iter().all(|o| o.tracker_id.is_none()) && s.list_orgs()?.is_empty() {
        return Ok(Vec::new());
    }
    Ok(out)
}

#[cfg(test)]
#[path = "orgs_tests.rs"]
mod tests;
