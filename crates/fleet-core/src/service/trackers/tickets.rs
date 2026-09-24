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
//! **The fence** (M3's decision 6, bounded by the org since M5). A host-bound
//! caller (an in-session Claude's per-host token) sees only the tracker
//! items linked to sessions on its own host whose org is the host's or
//! unassigned; a key or URL outside that is `E_FORBIDDEN` with one sentence
//! whether or not it exists, an item id outside it is "not found" exactly
//! as an unknown id is. Master and paired clients see everything.

use super::jira::parse_ticket_url;
use super::sync::fetch_one;
use super::ItemRef;
use super::TrackerNet;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
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

/// The items a caller may read, or `None` for all of them.
///
/// A per-host token (work graph M5, composing M3's interim fence — the plan's
/// decision 6 — with the org boundary): an item linked to a session on its
/// own host AND whose org is the host's or unassigned. The host fence is
/// kept because the org scope alone is wider (every host of an org would
/// read every ticket any of them works on); the org fence is added because
/// the host fence alone would let a host read another org's ticket that a
/// person force-linked on it.
fn allowed(scope: &OrgScope, s: &Store) -> Result<Option<HashSet<i64>>, IpcError> {
    let Some(h) = scope.host() else {
        return Ok(None);
    };
    let mut out = HashSet::new();
    for id in s.work_item_ids_on_host(h)? {
        if scope.sees_org(s.item_org(id)?) {
            out.insert(id);
        }
    }
    Ok(Some(out))
}

/// The live sessions working on `key` that the scope may see (D7: an
/// isolated org's session is not named to a host outside it).
fn live_ids(s: &Store, scope: &OrgScope, key: Option<&str>) -> Result<Vec<i64>, IpcError> {
    let Some(k) = key else {
        return Ok(Vec::new());
    };
    Ok(s.live_work_sessions_for_key(k)?
        .into_iter()
        .filter(|(_, r)| scope.sees_row(r))
        .map(|(_, r)| r.id)
        .collect())
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
    scope: &OrgScope,
) -> Result<Vec<Ticket>, IpcError> {
    let s = lock(store)?;
    let allowed = allowed(scope, &s)?;
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
        let live = live_ids(&s, scope, item.key.as_deref())?;
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
/// host-bound caller only those with a visible item linked on its host.
pub fn trackers(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<TrackerRow>, IpcError> {
    let s = lock(store)?;
    let all = s.list_trackers()?;
    let Some(allowed) = allowed(scope, &s)? else {
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

/// What a `lookup` reference names: a tracker and a key, when a URL (or a
/// reference only one tracker can answer) says which tracker; a bare key
/// otherwise.
fn recognise(s: &Store, reference: &str) -> Result<(Option<TrackerRow>, String), IpcError> {
    let r = reference.trim();
    let trackers = s.list_trackers()?;
    if r.starts_with("https://") || r.starts_with("http://") {
        if let Some((site, key)) = parse_ticket_url(r) {
            let t = trackers.into_iter().find(|t| t.site_url == site);
            if t.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("no tracker is connected for {site}; add it first (work_admin add)"),
                )
                .with_details(serde_json::json!({ "site_url": site, "key": key })));
            }
            return Ok((t, key));
        }
        // Any other tracker URL the recogniser knows (GitHub, Asana,
        // Linear): its reference, answered by the trackers that claim it.
        let m = crate::service::work::recognize::recognize(
            r,
            &crate::service::work::recognize::RecognizeCtx::default(),
        )
        .into_iter()
        .find(|m| m.kind == crate::service::work::recognize::MatchKind::Url)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "not a ticket URL fleet recognises (Jira, GitHub, Asana or Linear)",
            )
        })?;
        let key = crate::store::canonical_key(&m.key);
        let owners = crate::store::tracker_claims(&trackers, &key);
        if owners.is_empty() {
            let provider = m.provider.unwrap_or_default();
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no {provider} tracker is connected that covers {key}; add it first (work_admin add)"),
            )
            .with_details(serde_json::json!({ "provider": provider, "key": key, "url": r })));
        }
        let t = (owners.len() == 1)
            .then(|| trackers.iter().find(|t| t.id == owners[0]).cloned())
            .flatten();
        return Ok((t, key));
    }
    let key = crate::store::normalize_work_ref(r)?;
    // The tracker that may answer it, when exactly one does.
    let owners = crate::store::tracker_claims(&trackers, &key);
    let t = (owners.len() == 1)
        .then(|| trackers.iter().find(|t| t.id == owners[0]).cloned())
        .flatten();
    Ok((t, key))
}

/// `work { action: lookup, key | url }`: the cache, else one live fetch.
pub async fn lookup(
    store: &Mutex<Store>,
    reference: &str,
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<Ticket, IpcError> {
    let (tracker, key) = {
        let s = lock(store)?;
        match recognise(&s, reference) {
            Ok(r) => r,
            // Which sites are connected is not a host token's to learn: an
            // unknown site answers as an invisible key does.
            Err(e) if e.code == codes::E_NOTFOUND && scope.host().is_some() => {
                let h = scope.host().unwrap_or_default();
                return Err(orgs::not_visible_key(h, reference.trim()));
            }
            Err(e) => return Err(e),
        }
    };
    let cached = lock(store)?.tracker_item_for_key(&key)?;
    let item_id = match cached {
        Some(item) => item.id,
        None => {
            if let Some(h) = scope.host() {
                // Nothing cached is linked anywhere, let alone on this host;
                // do not let a host token make the hub fetch arbitrary keys.
                return Err(orgs::not_visible_key(h, &key));
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
            fetch_one(&t, ItemRef::parse(&key), store, net)
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
    if let Some(allowed) = allowed(scope, &s)? {
        if !allowed.contains(&item_id) {
            return Err(orgs::not_visible_key(
                scope.host().unwrap_or_default(),
                &key,
            ));
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
    let live = live_ids(&s, scope, item.key.as_deref())?;
    // An agent reads it: fenced on both sides, markers defused (M3 review).
    let description = meta.description.map(|d| match scope {
        OrgScope::Host { .. } => {
            crate::mcp::guard::fence_untrusted(&d, "a tracker ticket", super::DESCRIPTION_MAX_CHARS)
        }
        OrgScope::All => d,
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
    /// The session's name as a person edited it (default `KEY title`).
    pub name: Option<String>,
    /// The worktree (branch) name as a person edited it (default
    /// `slug(key + title)`); an existing worktree of that name is reused.
    pub worktree: Option<String>,
    /// Link across orgs anyway (work graph M5): the ticket's org differs
    /// from the org the new session would belong to.
    pub force_cross_org: bool,
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
    let raw = format!("{} {title}", slug_key(key)).to_lowercase();
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

/// The part of a key a branch name carries: a GitHub issue its number (the
/// repository is the project's), an Asana task the tail of its gid, a
/// ticket key itself.
fn slug_key(key: &str) -> String {
    if let Some((_, n)) = crate::store::github_ref(key) {
        return n.to_string();
    }
    if let Some(gid) = key.strip_prefix("asana:") {
        let tail: String = gid
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return tail;
    }
    key.to_string()
}

/// The friendly name `KEY title`, cut to the 80-character limit. An Asana
/// task has no human key: its title alone.
pub fn start_name(key: &str, title: &str) -> String {
    let t = if key.starts_with("asana:") && !title.trim().is_empty() {
        title.to_string()
    } else {
        format!("{key} {title}")
    };
    let t: String = t.chars().filter(|c| !c.is_control()).collect();
    t.trim().chars().take(80).collect()
}

/// Resolve the item a start names (cache, else live), then plan where it
/// lands. `E_EXISTS` (with the session) when the key already has a live
/// session; `E_AMBIGUOUS` (with candidates) when no project can be picked.
pub async fn plan_start(
    store: &Mutex<Store>,
    args: &StartArgs,
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<StartPlan, IpcError> {
    let (key, title, item_id) = match (args.item_id, args.reference.as_deref()) {
        (Some(id), None) => {
            let s = lock(store)?;
            // Out of scope reads exactly as unknown: no existence oracle.
            if allowed(scope, &s)?.is_some_and(|a| !a.contains(&id)) {
                return Err(orgs::not_found("work item", id));
            }
            let item = s
                .get_work_item(id)?
                .ok_or_else(|| orgs::not_found("work item", id))?;
            let key = item.key.clone().ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "that work item has no key to start from")
            })?;
            (key, item.title, Some(id))
        }
        (None, Some(r)) => match lookup(store, r, scope, net).await {
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
        if !scope.sees_row(&row) {
            // Still one live session per key, but an isolated org's session
            // is not named to a host outside it (D7).
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("{key} already has a live session"),
            ));
        }
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
    // Where this kind of work last ran: a GitHub issue's own repository's
    // project first, else the newest link with the same key prefix.
    let (seen, prefix_label) = match crate::store::github_ref(&key) {
        Some((repo, _)) => (
            s.project_for_repo(repo)?.map(|pid| {
                let host = s.last_host_for_project(pid).ok().flatten();
                (pid, host.unwrap_or_default())
            }),
            repo.to_string(),
        ),
        None => {
            let prefix = key
                .split_once('-')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            let label = if key.starts_with("asana:") {
                "this Asana task".to_string()
            } else {
                format!("{prefix}-*")
            };
            (s.last_place_for_prefix(&prefix)?, label)
        }
    };
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
                format!("no project has worked on {prefix_label} yet; pick one (project_id)"),
            )
            .with_details(serde_json::json!({ "candidates": candidates })));
        }
    };
    let host_alias = match (&args.host_alias, scope.host()) {
        (Some(h), _) => h.clone(),
        (None, Some(h)) => h.to_string(),
        (None, None) => match seen
            .filter(|p| p.0 == project_id && !p.1.is_empty())
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
    if let Some(h) = scope.host() {
        if host_alias != h {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!("a per-host token starts work only on its own host ({h})"),
            ));
        }
    }
    // Data integrity, for every caller (M5): a ticket of one org is not
    // attached to a session of another by mistake.
    if let Some(id) = item_id {
        let session_org = s.org_for_new_session(&host_alias, project_id)?;
        orgs::check_cross_org(s.item_org(id)?, session_org, &key, args.force_cross_org)?;
    }
    let branch = match args
        .worktree
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        Some(w) => {
            crate::validate::git_ref(w)?;
            if w == "main" || w == "master" {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "worktree name must not be 'main' or 'master'",
                ));
            }
            w.to_string()
        }
        None => branch_slug(&key, &title),
    };
    let worktree_id = s
        .list_worktrees_on_host(&host_alias)?
        .into_iter()
        .find(|w| w.project_id == project_id && w.name == branch)
        .map(|w| w.id);
    let name = match args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(n) => {
            crate::validate::friendly_name(n)?;
            n.to_string()
        }
        None => start_name(&key, &title),
    };
    Ok(StartPlan {
        name,
        key,
        title,
        item_id,
        project_id,
        host_alias,
        branch,
        worktree_id,
    })
}

/// One line of tracker text (a title, a status name) for fleet's own
/// lines: control characters and newlines flattened, markers defused, capped
/// — a newline in a Jira title must not be able to write a line of its own.
fn tracker_line(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::mcp::guard::defuse(&flat).chars().take(max).collect()
}

/// The ticket context a brief start queues: fleet's own lines (tracker text
/// in them flattened and defused), then the description fenced on both
/// sides. The description is what gets cut to fit
/// [`BRIEF_MAX_CHARS`](crate::service::work::handover::BRIEF_MAX_CHARS), so
/// the end marker always survives.
pub fn ticket_brief(store: &Mutex<Store>, plan: &StartPlan) -> Result<String, IpcError> {
    const FROM: &str = "the tracker ticket's description";
    let s = lock(store)?;
    let item = plan
        .item_id
        .map(|id| s.get_work_item(id))
        .transpose()?
        .flatten();
    let meta = plan.item_id.map(|id| s.work_item_meta(id)).transpose()?;
    let mut out = format!("You are starting work on {}", plan.key);
    if !plan.title.is_empty() {
        out.push_str(&format!(": {}", tracker_line(&plan.title, 200)));
    }
    out.push('\n');
    if let Some(i) = &item {
        if let Some(st) = &i.status_name {
            out.push_str(&format!("Status: {}\n", tracker_line(st, 80)));
        }
        if let Some(u) = &i.url {
            out.push_str(&format!("Ticket: {}\n", tracker_line(u, 300)));
        }
    }
    out.push_str(&format!("Branch: {}\n", plan.branch));
    if let Some(d) = meta.and_then(|m| m.description) {
        let overhead = out.chars().count()
            + crate::mcp::guard::fence_untrusted("", FROM, 0)
                .chars()
                .count()
            + 2;
        let budget = crate::service::work::handover::BRIEF_MAX_CHARS.saturating_sub(overhead);
        if budget > 0 {
            out.push('\n');
            out.push_str(&crate::mcp::guard::fence_untrusted(&d, FROM, budget));
            out.push('\n');
        }
    }
    Ok(out)
}

/// May the ticket's text reach a Claude on the plan's host? Only when the
/// item's org is inside that host's scope (M5).
pub fn brief_visible_on(store: &Mutex<Store>, plan: &StartPlan) -> Result<bool, IpcError> {
    let Some(id) = plan.item_id else {
        return Ok(true);
    };
    let s = lock(store)?;
    let target = OrgScope::for_host(&s, &plan.host_alias)?;
    Ok(target.sees_org(s.item_org(id)?))
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
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<SessionRow, IpcError> {
    let plan = plan_start(store, args, scope, net).await?;
    let brief = match (&args.brief, args.with_brief) {
        (Some(b), _) => Some(b.clone()),
        // The brief is read by the new session's Claude: never another
        // org's ticket text, even on a forced cross-org start.
        (None, true) if brief_visible_on(store, &plan)? => Some(ticket_brief(store, &plan)?),
        (None, _) => None,
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
