//! Work links (roadmap M1b.2): read and decide which work a session is doing.
//! Storage and its rules are in `store::work`; this is the one transport-
//! agnostic entry the MCP tools `work` / `work_link` and the desktop commands
//! share, so a paired desktop and a local one answer the same way.

pub mod agent_handover;
pub mod card;
pub mod detect;
pub mod handover;
pub mod harvest;
pub mod local;
pub mod nudge;
pub mod recognize;
pub mod resolve;
pub mod resume;
pub mod tidy;
pub mod today;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{SessionRow, Store, WorkLinkRow, WorkTarget};
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
    /// Lookup: a ticket URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Today: unix start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
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
    /// Snooze (7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// tidy_apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<tidy::TidyApplyItem>>,
    /// Approved nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
    /// Name: the work's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
    /// Tidy-up candidates (work graph M7).
    Tidy,
    /// Work open again that has past sessions (work graph M7).
    Reopened,
    /// Local work items: named work with no ticket (work graph M11.1).
    LocalItems,
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
    ("tidy", WorkAction::Tidy),
    ("reopened", WorkAction::Reopened),
    ("local_items", WorkAction::LocalItems),
];

/// Every `work_link` action. The tool refuses any other name before
/// dispatching, so an action cannot be added without appearing here — and
/// so in the isolation matrix.
pub const WORK_LINK_ACTIONS: &[&str] = &[
    "link",
    "reject",
    "unlink",
    "confirm",
    "trust_project",
    "resume",
    "start",
    "handover",
    "archive",
    "unarchive",
    "snooze",
    "never",
    "dismiss",
    "tidy_apply",
    "name",
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
    ("work_scopes", "work", "scopes"),
    ("list_orgs", "work", "orgs"),
    ("org_suggestions", "work", "org_suggestions"),
    ("work_today", "work", "today"),
    ("work_ticket_card", "work", "card"),
    ("link_session_work", "work_link", "link"),
    ("reject_session_work", "work_link", "reject"),
    ("unlink_session_work", "work_link", "unlink"),
    ("confirm_session_work", "work_link", "confirm"),
    ("set_work_project_trust", "work_link", "trust_project"),
    ("resume_work", "work_link", "resume"),
    ("start_work", "work_link", "start"),
    ("request_work_handover", "work_link", "handover"),
    ("start_work_multi", "work_link", "start"),
    ("work_tidy", "work", "tidy"),
    ("work_reopened", "work", "reopened"),
    ("archive_session_work", "work_link", "archive"),
    ("unarchive_session_work", "work_link", "unarchive"),
    ("snooze_tidy", "work_link", "snooze"),
    ("never_tidy", "work_link", "never"),
    ("tidy_apply", "work_link", "tidy_apply"),
    ("dismiss_reopened", "work_link", "dismiss"),
    ("list_local_work_items", "work", "local_items"),
    ("name_session_work", "work_link", "name"),
    ("rename_work_item", "work_link", "name"),
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
    scope: &OrgScope,
) -> Result<WorkContext, IpcError> {
    let key = crate::store::normalize_work_ref(args.required_key()?)?;
    orgs::require_key(&*lock(store)?, scope, &key)?;
    let input = handover::gather_handover(store, ssh.as_ref(), &key, None, scope).await?;
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
    scope: &OrgScope,
) -> Result<resume::ResumePlan, IpcError> {
    resume::resume_plan(
        store,
        ssh,
        args.required_key()?,
        args.link_id,
        args.host_alias.as_deref(),
        args.with_brief.unwrap_or(false),
        scope,
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
) -> Result<SessionRow, IpcError> {
    resume::resume_work(store, ssh, reg, &resume_args(args)?, scope).await
}

/// `work { action: lookup }`'s reference: `url`, else `key`.
pub fn lookup_reference(args: &WorkArgs) -> Result<&str, IpcError> {
    args.url
        .as_deref()
        .or(args.key.as_deref())
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "lookup needs key or url"))
}

/// The start half of [`WorkLinkArgs`] (work graph M3.4).
pub fn start_args(args: &WorkLinkArgs) -> crate::service::trackers::tickets::StartArgs {
    crate::service::trackers::tickets::StartArgs {
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
    }
}

/// The resume half of [`WorkLinkArgs`].
pub fn resume_args(args: &WorkLinkArgs) -> Result<resume::ResumeArgs, IpcError> {
    Ok(resume::ResumeArgs {
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
/// primary first); `{key}` → ended links to the key (past work). Exactly one.
/// A per-host token reads only links inside its orgs, and past links only of
/// its own host's sessions (`orgs::scope_links`).
pub fn work(
    args: &WorkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<Vec<WorkLinkRow>, IpcError> {
    let s = lock(store)?;
    let mut links = work_unscoped(args, &s)?;
    orgs::scope_links(&s, scope, &mut links)?;
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
pub fn work_link<'a>(
    args: &'a WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<SessionRow, IpcError> {
    if matches!(
        args.action.as_str(),
        "resume" | "start" | "trust_project" | "handover" | "dismiss" | "tidy_apply" | "name"
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
    let s = lock(store)?;
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
    // The lifecycle actions (work graph M7) write flags, not decisions: no
    // resolver run after them.
    match args.action.as_str() {
        "archive" => {
            s.archive_session_work(session_id)?;
            return lifecycle_row(&s, session_id);
        }
        // A click, or an attach: a person's touch un-archives.
        "unarchive" => {
            if !s.touch_session(session_id)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} not found"),
                ));
            }
            return lifecycle_row(&s, session_id);
        }
        "snooze" => {
            if let Some(l) = args.link_id {
                visible_link(l)?;
            }
            let days = args.days.unwrap_or(tidy::SNOOZE_DEFAULT_DAYS);
            if !(1..=tidy::SNOOZE_MAX_DAYS).contains(&days) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("snooze days must be 1..={}", tidy::SNOOZE_MAX_DAYS),
                ));
            }
            let until = crate::service::catalog::now_secs() + i64::from(days) * 86_400;
            s.snooze_tidy(session_id, args.link_id, until)?;
            return lifecycle_row(&s, session_id);
        }
        "never" => {
            if let Some(l) = args.link_id {
                visible_link(l)?;
            }
            s.never_tidy(session_id, args.link_id)?;
            return lifecycle_row(&s, session_id);
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
        "link" => {
            let source = args.source.as_deref().unwrap_or("manual");
            let (t, org) = visible_target(target()?)?;
            orgs::check_cross_org(org, s.session_org(session_id)?, &target_name(t), force)?;
            if source == AGENT_INFERRED {
                // The classification nudge's answer (work graph M4.6): a
                // guess, so a pre-selected suggestion (R11), never a link.
                let (key, tracker) = inferred_target(&s, t)?;
                detect::on_agent_inference(&s, session_id, &key, tracker)?;
            } else {
                s.link_session_work(session_id, t, source)?;
            }
        }
        // `reject { link_id }` decides one suggestion (work graph M4.4);
        // `reject { key | item_id }` any target.
        "reject" if args.link_id.is_some() && args.key.is_none() && args.item_id.is_none() => {
            let link_id = args.link_id.unwrap_or_default();
            if !scope.is_all() {
                visible_link(link_id)?;
            }
            detect::decide(&s, session_id, link_id, false)?;
        }
        "reject" => {
            let (t, _) = visible_target(target()?)?;
            s.reject_session_work(session_id, t)?;
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
            } else if !scope.is_all() {
                visible_link(link_id)?;
            }
            detect::decide(&s, session_id, link_id, true)?;
        }
        "unlink" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "unlink needs link_id"))?;
            if !scope.is_all() {
                visible_link(link_id)?;
            }
            if !s.unlink_session_work(session_id, link_id)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} has no live work link {link_id}"),
                ));
            }
        }
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
    if let Err(e) = detect::resolve_session(&s, session_id) {
        tracing::debug!(error = %e.message, "[work] resolve after a decision failed");
    }
    s.get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

fn lifecycle_row(s: &Store, session_id: i64) -> Result<SessionRow, IpcError> {
    s.get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

/// `work_link { action: link, source }`'s value for the agent's answer to
/// the classification nudge (work graph M4.6).
pub const AGENT_INFERRED: &str = "agent_inferred";

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
            &OrgScope::All,
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
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(links[0].state, "rejected");
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
            work(&WorkArgs::default(), &st, &OrgScope::All)
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
            work(&both, &st, &OrgScope::All).unwrap_err().code,
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
