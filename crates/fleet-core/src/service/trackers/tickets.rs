//! Reading tickets and starting work on one (work graph M3.4).
//!
//! * `work { action: tickets }` serves the **cache** — never a tracker call —
//!   filtered by view. The built-in views are evaluated locally from what
//!   the sync stored (assignee, status, sprint, `updated`), so an item that
//!   left "My work" leaves it here on the next sync even though an
//!   incremental listing never says so; favourite filters use the
//!   membership the sync recorded.
//! * `work { action: lookup }` answers from the cache, or fetches one item
//!   live and caches it. A URL is recognised against the trackers' sites.
//! * `work_link { action: start }` is the compound "start work on this
//!   ticket": resolve, refuse a duplicate (`E_EXISTS` with the live
//!   session, so the UI jumps to it), pick the project and host, name the
//!   worktree after the key and title, create the session, link it
//!   `started`, and — with a brief — queue the ticket's context (third-party
//!   text inside `mark_untrusted`) for the first hook.
//!
//! **The interim fence (plan decision 6).** Until orgs exist, a host-bound
//! caller (an in-session Claude's per-host token) sees only the tracker
//! items linked to sessions on its own host; anything else is `E_FORBIDDEN`
//! with a sentence that says why. Master and paired clients see everything.

use super::jira::parse_ticket_url;
use super::sync::fetch_one;
use super::ItemRef;
use crate::ipc_error::{codes, lock, IpcError};
use crate::net::https::HttpTransport;
use crate::store::{SessionRow, Store, TrackerRow, WorkItemRow, WorkTarget};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Default and maximum rows one `tickets` read returns.
pub const TICKETS_DEFAULT_LIMIT: usize = 50;
pub const TICKETS_MAX_LIMIT: usize = 200;
/// The `recent` view's window.
pub const RECENT_DAYS: i64 = 14;

/// One ticket as `tickets` and `lookup` return it: the item, and the live
/// sessions already working on it (Enter jumps instead of starting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    #[serde(flatten)]
    pub item: WorkItemRow,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_session_ids: Vec<i64>,
    /// `lookup` only: the description excerpt. Third-party text; for an
    /// in-session agent it arrives inside the untrusted-input marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `lookup` only: the item's views (`mine`, `sprint`, `recent`,
    /// `filter:<id>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
}

/// Who is asking, for the fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// Master or a paired client: everything.
    All,
    /// A per-host token: items linked to sessions on this host only.
    Host(&'a str),
}

impl Scope<'_> {
    fn allowed(&self, s: &Store) -> Result<Option<HashSet<i64>>, IpcError> {
        match self {
            Scope::All => Ok(None),
            Scope::Host(h) => Ok(Some(s.work_item_ids_on_host(h)?.into_iter().collect())),
        }
    }
}

fn forbidden(host: &str, what: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{what} is not linked to any session on {host}; a per-host token sees only the \
             tickets its own host's sessions work on (tracker isolation before orgs)"
        ),
    )
}

fn views_of(
    item: &WorkItemRow,
    meta: &crate::store::ItemMeta,
    t: Option<&TrackerRow>,
    now: i64,
) -> Vec<String> {
    let mine = t
        .and_then(|t| t.config.account_id.as_deref())
        .is_some_and(|me| meta.assignee_id.as_deref() == Some(me));
    let mut v = Vec::new();
    if mine && item.status_category != "done" && item.unavailable_at.is_none() {
        v.push("mine".to_string());
    }
    if mine && meta.iteration_active && item.unavailable_at.is_none() {
        v.push("sprint".to_string());
    }
    if mine
        && item
            .updated_ext
            .is_some_and(|u| u >= now - RECENT_DAYS * 86_400)
    {
        v.push("recent".to_string());
    }
    v.extend(meta.views.iter().cloned());
    v
}

/// `work { action: tickets, tracker_id?, view?, query?, limit? }`.
pub fn tickets(
    store: &Mutex<Store>,
    tracker_id: Option<i64>,
    view: Option<&str>,
    query: Option<&str>,
    limit: Option<usize>,
    scope: Scope<'_>,
) -> Result<Vec<Ticket>, IpcError> {
    let s = lock(store)?;
    let allowed = scope.allowed(&s)?;
    let trackers = s.list_trackers()?;
    let now = crate::service::catalog::now_secs();
    let q = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let limit = limit
        .unwrap_or(TICKETS_DEFAULT_LIMIT)
        .clamp(1, TICKETS_MAX_LIMIT);
    let mut out = Vec::new();
    for (item, meta) in s.tracker_items(tracker_id)? {
        if allowed.as_ref().is_some_and(|a| !a.contains(&item.id)) {
            continue;
        }
        let t = trackers.iter().find(|t| Some(t.id) == item.tracker_id);
        if let Some(view) = view {
            if !views_of(&item, &meta, t, now).iter().any(|v| v == view) {
                continue;
            }
        }
        if let Some(q) = &q {
            let hay = format!(
                "{} {} {}",
                item.key.as_deref().unwrap_or_default(),
                item.title,
                item.assignees.join(" ")
            )
            .to_lowercase();
            if !hay.contains(q.as_str()) && !item.aliases.iter().any(|a| a.to_lowercase() == *q) {
                continue;
            }
        }
        let live = match item.key.as_deref() {
            Some(k) => s
                .live_work_sessions_for_key(k)?
                .into_iter()
                .map(|(_, r)| r.id)
                .collect(),
            None => Vec::new(),
        };
        out.push(Ticket {
            item,
            live_session_ids: live,
            description: None,
            views: Vec::new(),
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// `work { action: trackers }`: every tracker (no secrets), or for a
/// host-bound caller only those with an item linked on its host.
pub fn trackers(store: &Mutex<Store>, scope: Scope<'_>) -> Result<Vec<TrackerRow>, IpcError> {
    let s = lock(store)?;
    let all = s.list_trackers()?;
    let Some(allowed) = scope.allowed(&s)? else {
        return Ok(all);
    };
    let mut ids = HashSet::new();
    for id in allowed {
        if let Some(t) = s.get_work_item(id)?.and_then(|i| i.tracker_id) {
            ids.insert(t);
        }
    }
    Ok(all.into_iter().filter(|t| ids.contains(&t.id)).collect())
}

/// What a `lookup` reference names: a tracker and a key, when a URL says
/// which tracker; a bare key otherwise.
fn recognise(s: &Store, reference: &str) -> Result<(Option<TrackerRow>, String), IpcError> {
    let r = reference.trim();
    if r.starts_with("https://") || r.starts_with("http://") {
        let (site, key) = parse_ticket_url(r).ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "not a ticket URL fleet recognises (https://<site>.atlassian.net/browse/KEY-1)",
            )
        })?;
        let t = s.list_trackers()?.into_iter().find(|t| t.site_url == site);
        if t.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no tracker is connected for {site}; add it first (work_admin add)"),
            )
            .with_details(serde_json::json!({ "site_url": site, "key": key })));
        }
        return Ok((t, key));
    }
    let key = crate::store::normalize_work_ref(r)?;
    // The tracker that owns the prefix, when exactly one does.
    let prefix = key
        .split_once('-')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default();
    let owners: Vec<TrackerRow> = s
        .list_trackers()?
        .into_iter()
        .filter(|t| t.config.key_prefixes.contains(&prefix))
        .collect();
    Ok(((owners.len() == 1).then(|| owners[0].clone()), key))
}

/// `work { action: lookup, key | url }`: the cache, else one live fetch.
pub async fn lookup(
    store: &Mutex<Store>,
    reference: &str,
    scope: Scope<'_>,
    transport: Arc<dyn HttpTransport>,
) -> Result<Ticket, IpcError> {
    let (tracker, key) = {
        let s = lock(store)?;
        recognise(&s, reference)?
    };
    let cached = lock(store)?.tracker_item_for_key(&key)?;
    let item_id = match cached {
        Some(item) => item.id,
        None => {
            if let Scope::Host(h) = scope {
                // Nothing cached is linked anywhere, let alone on this host;
                // do not let a host token make the hub fetch arbitrary keys.
                return Err(forbidden(h, &key));
            }
            let t = tracker.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("{key} is not a ticket of any connected tracker"),
                )
            })?;
            if !super::sync::runnable(&t.state) {
                return Err(IpcError::new(
                    codes::E_TRACKER,
                    format!(
                        "{key} is not cached and {} cannot be asked now (state {})",
                        t.name, t.state
                    ),
                )
                .with_details(serde_json::json!({ "state": t.state })));
            }
            fetch_one(&t, ItemRef::Key(key.clone()), store, transport)
                .await
                .map_err(|e| e.to_ipc())?
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_NOTFOUND,
                        format!("{key} was not found, or is not visible to the tracker's account"),
                    )
                })?
        }
    };
    let s = lock(store)?;
    if let Some(allowed) = scope.allowed(&s)? {
        if !allowed.contains(&item_id) {
            let Scope::Host(h) = scope else {
                unreachable!()
            };
            return Err(forbidden(h, &key));
        }
    }
    let item = s
        .get_work_item(item_id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "item vanished"))?;
    let meta = s.work_item_meta(item_id)?;
    let t = s
        .list_trackers()?
        .into_iter()
        .find(|t| Some(t.id) == item.tracker_id);
    let views = views_of(
        &item,
        &meta,
        t.as_ref(),
        crate::service::catalog::now_secs(),
    );
    let live = match item.key.as_deref() {
        Some(k) => s
            .live_work_sessions_for_key(k)?
            .into_iter()
            .map(|(_, r)| r.id)
            .collect(),
        None => Vec::new(),
    };
    let description = meta.description.map(|d| match scope {
        Scope::Host(_) => crate::mcp::guard::mark_untrusted(&d, "a tracker ticket"),
        Scope::All => d,
    });
    Ok(Ticket {
        item,
        live_session_ids: live,
        description,
        views,
    })
}

// --- start --------------------------------------------------------------------

/// `work_link { action: start }`'s arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartArgs {
    /// Key, URL, or …
    pub reference: Option<String>,
    /// … an item id.
    pub item_id: Option<i64>,
    pub project_id: Option<i64>,
    pub host_alias: Option<String>,
    /// Queue the ticket's context for the first hook.
    pub with_brief: bool,
    /// The brief as a person edited it (implies `with_brief`).
    pub brief: Option<String>,
}

/// Where a start lands and what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPlan {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub item_id: Option<i64>,
    pub project_id: i64,
    pub host_alias: String,
    /// The worktree (and branch) name: `slug(key + " " + title)`.
    pub branch: String,
    /// An existing worktree of that name on the host, reused.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// `KEY title`, the session's friendly name.
    pub name: String,
}

/// `slug(key + " " + title)`: lower case, `[a-z0-9-]`, runs collapsed, at
/// most 60 characters, never `main`/`master`. The frontend's
/// `finalizeBranchSlug` does the same for what a person types.
pub fn branch_slug(key: &str, title: &str) -> String {
    let raw = format!("{key} {title}").to_lowercase();
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut out: String = out.chars().take(60).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() || out == "main" || out == "master" {
        out = format!("work-{}", key.to_lowercase());
    }
    out
}

/// The friendly name `KEY title`, cut to the 80-character limit.
pub fn start_name(key: &str, title: &str) -> String {
    let t = format!("{key} {title}");
    let t: String = t.chars().filter(|c| !c.is_control()).collect();
    t.trim().chars().take(80).collect()
}

/// Resolve the item a start names (cache, else live), then plan where it
/// lands. `E_EXISTS` (with the session) when the key already has a live
/// session; `E_AMBIGUOUS` (with candidates) when no project can be picked.
pub async fn plan_start(
    store: &Mutex<Store>,
    args: &StartArgs,
    scope: Scope<'_>,
    transport: Arc<dyn HttpTransport>,
) -> Result<StartPlan, IpcError> {
    let (key, title, item_id) = match (args.item_id, args.reference.as_deref()) {
        (Some(id), None) => {
            let s = lock(store)?;
            let item = s.get_work_item(id)?.ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("work item {id} not found"))
            })?;
            if let Some(allowed) = scope.allowed(&s)? {
                if !allowed.contains(&id) {
                    let Scope::Host(h) = scope else {
                        unreachable!()
                    };
                    return Err(forbidden(h, &format!("work item {id}")));
                }
            }
            let key = item.key.clone().ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "that work item has no key to start from")
            })?;
            (key, item.title, Some(id))
        }
        (None, Some(r)) => match lookup(store, r, scope, transport).await {
            Ok(t) => (
                t.item.key.clone().unwrap_or_else(|| r.to_string()),
                t.item.title,
                Some(t.item.id),
            ),
            // A key no tracker knows still starts work (trackers never gate).
            Err(e) if e.code == codes::E_NOTFOUND && !r.contains("://") => {
                (crate::store::normalize_work_ref(r)?, String::new(), None)
            }
            Err(e) => return Err(e),
        },
        _ => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "start needs exactly one of key, url or item_id",
            ))
        }
    };
    let s = lock(store)?;
    if let Some((_, row)) = s.live_work_sessions_for_key(&key)?.into_iter().next() {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "{key} already has a live session ({} on {}); jump to it",
                row.friendly_name.as_deref().unwrap_or(&row.tmux_name),
                row.host_alias
            ),
        )
        .with_details(serde_json::json!({
            "session_id": row.id,
            "host_alias": row.host_alias,
            "tmux_name": row.tmux_name,
        })));
    }
    let prefix = key
        .split_once('-')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default();
    let seen = s.last_place_for_prefix(&prefix)?;
    let project_id = match args.project_id.or(seen.as_ref().map(|p| p.0)) {
        Some(pid) => {
            if !s.list_projects()?.iter().any(|p| p.id == pid) {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("project {pid} not found"),
                ));
            }
            pid
        }
        None => {
            let mut projects = s.list_projects()?;
            projects.retain(|p| !p.system);
            projects.sort_by_key(|p| std::cmp::Reverse(p.last_session_at.unwrap_or(0)));
            let candidates: Vec<serde_json::Value> = projects
                .iter()
                .take(8)
                .map(|p| serde_json::json!({ "id": p.id, "owner": p.owner, "repo": p.repo }))
                .collect();
            return Err(IpcError::new(
                codes::E_AMBIGUOUS,
                format!("no project has worked on {prefix}-* yet; pick one (project_id)"),
            )
            .with_details(serde_json::json!({ "candidates": candidates })));
        }
    };
    let host_alias = match (&args.host_alias, &scope) {
        (Some(h), _) => h.clone(),
        (None, Scope::Host(h)) => h.to_string(),
        (None, Scope::All) => match seen
            .filter(|p| p.0 == project_id)
            .map(|p| p.1)
            .or(s.last_host_for_project(project_id)?)
        {
            Some(h) => h,
            None => {
                let hosts: Vec<String> = s
                    .list_hosts()?
                    .into_iter()
                    .filter(|h| h.reachable)
                    .map(|h| h.alias)
                    .collect();
                return Err(IpcError::new(
                    codes::E_AMBIGUOUS,
                    "no host has run this project yet; pick one (host_alias)",
                )
                .with_details(serde_json::json!({ "candidates": hosts })));
            }
        },
    };
    if let Scope::Host(h) = scope {
        if host_alias != h {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!("a per-host token starts work only on its own host ({h})"),
            ));
        }
    }
    let branch = branch_slug(&key, &title);
    let worktree_id = s
        .list_worktrees_on_host(&host_alias)?
        .into_iter()
        .find(|w| w.project_id == project_id && w.name == branch)
        .map(|w| w.id);
    Ok(StartPlan {
        name: start_name(&key, &title),
        key,
        title,
        item_id,
        project_id,
        host_alias,
        branch,
        worktree_id,
    })
}

/// The ticket context a brief start queues: fleet's own lines, then the
/// description inside the untrusted-input marker.
pub fn ticket_brief(store: &Mutex<Store>, plan: &StartPlan) -> Result<String, IpcError> {
    let s = lock(store)?;
    let item = plan
        .item_id
        .map(|id| s.get_work_item(id))
        .transpose()?
        .flatten();
    let meta = plan.item_id.map(|id| s.work_item_meta(id)).transpose()?;
    let mut out = format!("You are starting work on {}", plan.key);
    if !plan.title.is_empty() {
        out.push_str(&format!(": {}", plan.title));
    }
    out.push('\n');
    if let Some(i) = &item {
        if let Some(st) = &i.status_name {
            out.push_str(&format!("Status: {st}\n"));
        }
        if let Some(u) = &i.url {
            out.push_str(&format!("Ticket: {u}\n"));
        }
    }
    out.push_str(&format!("Branch: {}\n", plan.branch));
    if let Some(d) = meta.and_then(|m| m.description) {
        out.push('\n');
        out.push_str(&crate::mcp::guard::mark_untrusted(
            &d,
            "the tracker ticket's description",
        ));
        out.push('\n');
        out.push_str(crate::mcp::guard::UNTRUSTED_END);
        out.push('\n');
    }
    Ok(out
        .chars()
        .take(crate::service::work::handover::BRIEF_MAX_CHARS)
        .collect())
}

/// The short prompt typed once the REPL is ready (the brief rides the
/// hook's `additionalContext`).
pub fn start_prompt(key: &str) -> String {
    format!("Start on {key}: the ticket's context is in your fleet brief. Read it, then plan before you edit.")
}

/// Do the start. `spawn` makes the session (production: `new_session`).
/// Returns the new row, linked `started`, and whether a brief was queued.
pub async fn start_with<F, Fut>(
    store: &Arc<Mutex<Store>>,
    plan: &StartPlan,
    brief: Option<String>,
    spawn: F,
) -> Result<(SessionRow, bool), IpcError>
where
    F: FnOnce(crate::service::sessions::NewSessionArgs) -> Fut,
    Fut: std::future::Future<Output = Result<SessionRow, IpcError>>,
{
    let args = crate::service::sessions::NewSessionArgs {
        host_alias: plan.host_alias.clone(),
        project_id: plan.project_id,
        worktree_id: plan.worktree_id,
        name: String::new(),
        call_id: None,
        new_worktree: plan.worktree_id.is_none().then(|| plan.branch.clone()),
        base_branch: None,
        kind: None,
        start_command: None,
        friendly_name: crate::validate::friendly_name(&plan.name)
            .is_ok()
            .then(|| plan.name.clone()),
        resume_claude_session_id: None,
    };
    let row = spawn(args).await?;
    let s = lock(store)?;
    let target = match plan.item_id {
        Some(id) => WorkTarget::Item(id),
        None => WorkTarget::Key(&plan.key),
    };
    s.link_session_work(row.id, target, "started")?;
    let queued = match brief {
        Some(body) if !body.trim().is_empty() => {
            let body: String = body
                .chars()
                .take(crate::service::work::handover::BRIEF_MAX_CHARS)
                .collect();
            let meta =
                serde_json::json!({ "key": plan.key, "item_id": plan.item_id, "source": "start" })
                    .to_string();
            s.enqueue_handover(row.id, &body, Some(&meta))?;
            true
        }
        _ => false,
    };
    let row = s.get_session_by_id(row.id)?.unwrap_or(row);
    Ok((row, queued))
}

/// `work_link { action: start }` end to end over the real `new_session`.
pub async fn start_work(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    args: &StartArgs,
    scope: Scope<'_>,
    transport: Arc<dyn HttpTransport>,
) -> Result<SessionRow, IpcError> {
    let plan = plan_start(store, args, scope, transport).await?;
    let brief = match (&args.brief, args.with_brief) {
        (Some(b), _) => Some(b.clone()),
        (None, true) => Some(ticket_brief(store, &plan)?),
        (None, false) => None,
    };
    let (row, queued) = start_with(store, &plan, brief, |a| {
        crate::service::sessions::new_session(a, store.as_ref(), ssh, reg)
    })
    .await?;
    if queued {
        crate::service::work::resume::spawn_start_prompt(
            Arc::clone(store),
            Arc::clone(ssh),
            &row,
            start_prompt(&plan.key),
        );
    }
    Ok(row)
}

#[cfg(test)]
#[path = "tests_tickets.rs"]
mod tests;
