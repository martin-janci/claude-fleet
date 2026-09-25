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
use crate::store::{SessionRow, Store, TrackerRow, WorkItemRow, WorkLinkRow, WorkTarget};
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
pub(crate) fn allowed(scope: &OrgScope, s: &Store) -> Result<Option<HashSet<i64>>, IpcError> {
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

/// The live sessions working on `key` as far as the item of `item_org` is
/// concerned. A link whose session is in one org while the item is in
/// another — a bare `ref_key` that `work_link` deliberately kept bare
/// because the key was outside the caller's orgs (work graph M5) — is not
/// live work on that item: a host of org A must not be able to block org
/// B's `start` or be named as working on B's ticket by typing its key. The
/// rule is [`Store::bind_tracker_refs`]'s: an unassigned side never
/// conflicts, and a forced item link keeps its item's org.
fn live_work_on(
    s: &Store,
    key: &str,
    item_org: Option<i64>,
) -> Result<Vec<(WorkLinkRow, SessionRow)>, IpcError> {
    let mut out = Vec::new();
    for (l, row) in s.live_work_sessions_for_key(key)? {
        if let (Some(io), Some(lo)) = (item_org, s.link_org(&l)?) {
            if io != lo {
                continue;
            }
        }
        out.push((l, row));
    }
    Ok(out)
}

/// The live sessions working on `item` that the scope may see (D7: an
/// isolated org's session is not named to a host outside it).
fn live_ids(s: &Store, scope: &OrgScope, item: &WorkItemRow) -> Result<Vec<i64>, IpcError> {
    let Some(k) = item.key.as_deref() else {
        return Ok(Vec::new());
    };
    Ok(live_work_on(s, k, s.item_org(item.id)?)?
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
    tickets_in(&s, tracker_id, view, query, limit, scope)
}

/// [`tickets`] under a store guard the caller already holds — the hook path
/// (the classification nudge, work graph M4.6) answers from inside one.
pub(crate) fn tickets_in(
    s: &Store,
    tracker_id: Option<i64>,
    view: Option<&str>,
    query: Option<&str>,
    limit: Option<usize>,
    scope: &OrgScope,
) -> Result<Vec<Ticket>, IpcError> {
    let allowed = allowed(scope, s)?;
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
        let live = live_ids(s, scope, &item)?;
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
/// otherwise. The flag says the reference was a URL, whose tracker is
/// then the only cache to answer from.
fn recognise(s: &Store, reference: &str) -> Result<(Option<TrackerRow>, String, bool), IpcError> {
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
            return Ok((t, key, true));
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
        return Ok((t, key, true));
    }
    let key = crate::store::normalize_work_ref(r)?;
    // The tracker that may answer it, when exactly one does.
    let owners = crate::store::tracker_claims(&trackers, &key);
    let t = (owners.len() == 1)
        .then(|| trackers.iter().find(|t| t.id == owners[0]).cloned())
        .flatten();
    Ok((t, key, false))
}

/// `work { action: lookup, key | url }`: the cache, else one live fetch.
pub async fn lookup(
    store: &Mutex<Store>,
    reference: &str,
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<Ticket, IpcError> {
    let (tracker, key, by_url) = {
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
    // A URL names its tracker: only that tracker's cache answers for it
    // (two sites can share a project key). A bare key asks every cache.
    let cached = match (&tracker, by_url) {
        (Some(t), true) => lock(store)?.tracker_item_for_key_in(t.id, &key)?,
        _ => lock(store)?.tracker_item_for_key(&key)?,
    };
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
            // Inside a 429's Retry-After the sync recorded: no request, the
            // same refusal, and how long is left — a lookup must not extend
            // the outage the sync is waiting out.
            let now = crate::service::catalog::now_secs();
            if let Some(nb) = lock(store)?
                .tracker_not_before(t.id)?
                .filter(|nb| *nb > now)
            {
                let left = nb - now;
                return Err(IpcError::new(
                    codes::E_TRACKER,
                    format!(
                        "{key} is not cached and {} cannot be asked now (rate-limited for \
                         another {left}s)",
                        t.name
                    ),
                )
                .with_details(serde_json::json!({ "state": t.state, "retry_after_secs": left })));
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
    let live = live_ids(&s, scope, &item)?;
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
    /// A multi-repo start (work graph M9.6): the duplicate guard counts only
    /// live sessions on the key in THIS start's project.
    pub per_project: bool,
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
    /// A multi-repo start's sibling: the duplicate guard counts only live
    /// sessions on the key in this plan's project.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub per_project: bool,
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
    let ticket = resolve_start(store, args, scope, net).await?;
    plan_resolved(store, args, scope, &ticket)
}

/// The ticket a start names, resolved once: its key, title and (when a
/// tracker knows it) item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartTicket {
    pub key: String,
    pub title: String,
    pub item_id: Option<i64>,
}

/// Resolve the item a start names: the cache, else one live lookup. A key
/// no tracker knows still starts work (trackers never gate).
pub async fn resolve_start(
    store: &Mutex<Store>,
    args: &StartArgs,
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<StartTicket, IpcError> {
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
    Ok(StartTicket {
        key,
        title,
        item_id,
    })
}

/// A per-host token starting work on another host.
fn host_fence(h: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!("a per-host token starts work only on its own host ({h})"),
    )
}

/// `E_EXISTS` naming `row` (or not, when the scope cannot see it — an
/// isolated org's session is not named to a host outside it, D7). `what`
/// qualifies the session ("on branch x"); `then` is what to do about it.
fn already_running(
    key: &str,
    row: &SessionRow,
    scope: &OrgScope,
    what: &str,
    then: &str,
) -> IpcError {
    if !scope.sees_row(row) {
        return IpcError::new(codes::E_EXISTS, format!("{key} already has a live session"));
    }
    IpcError::new(
        codes::E_EXISTS,
        format!(
            "{key} already has a live session{what} ({} on {}); {then}",
            row.friendly_name.as_deref().unwrap_or(&row.tmux_name),
            row.host_alias
        ),
    )
    .with_details(serde_json::json!({
        "session_id": row.id,
        "host_alias": row.host_alias,
        "tmux_name": row.tmux_name,
    }))
}

/// Plan where a resolved ticket's start lands. `E_EXISTS` (with the
/// session) when the key already has a live session — in THIS project for a
/// multi-repo start, where a live session on the start's branch counts too,
/// linked or not (a sibling that spawned but failed to link, or a person's
/// own session there); `E_AMBIGUOUS` (with candidates) when no project can
/// be picked.
pub fn plan_resolved(
    store: &Mutex<Store>,
    args: &StartArgs,
    scope: &OrgScope,
    ticket: &StartTicket,
) -> Result<StartPlan, IpcError> {
    let StartTicket {
        key,
        title,
        item_id,
    } = ticket.clone();
    // The host fence answers first: a per-host token asking for another
    // host learns nothing about the sessions there.
    if let (Some(h), Some(asked)) = (scope.host(), args.host_alias.as_deref()) {
        if asked != h {
            return Err(host_fence(h));
        }
    }
    let s = lock(store)?;
    let item_org = item_id.map(|id| s.item_org(id)).transpose()?.flatten();
    let live = live_work_on(&s, &key, item_org)?;
    if let Some((_, row)) = live
        .iter()
        .find(|(_, r)| !args.per_project || r.project_id == args.project_id)
    {
        return Err(already_running(&key, row, scope, "", "jump to it"));
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
            return Err(host_fence(h));
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
    if args.per_project {
        // A live session already on this branch in this project, linked or
        // not: a retry after a spawn whose link failed must not start a
        // second one on the same checkout. A fresh worktree has no row until
        // the next scan, so its sessions are matched by their worktree key.
        if let Some(row) = s.list_sessions_for_host(&host_alias)?.iter().find(|r| {
            r.project_id == Some(project_id)
                && r.status == "running"
                && r.lost_at.is_none()
                && ((worktree_id.is_some() && r.worktree_id == worktree_id)
                    || r.worktree_key.as_deref() == Some(branch.as_str()))
        }) {
            return Err(already_running(
                &key,
                row,
                scope,
                &format!(" on branch {branch}"),
                "jump to it",
            ));
        }
    }
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
        per_project: args.per_project,
    })
}

/// Most characters of a key fleet's own lines carry.
const KEY_LINE_MAX: usize = 64;

/// One line of tracker text (a title, a status name, a key) for fleet's
/// own lines: control characters and newlines flattened, markers defused,
/// capped — a newline in a Jira title must not be able to write a line of
/// its own.
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
    ticket_brief_with(store, plan, "")
}

/// [`ticket_brief`] with more of fleet's own lines (`extra`, e.g. a
/// multi-repo start's siblings) after the header, inside the same budget,
/// so the fence's end always survives.
pub fn ticket_brief_with(
    store: &Mutex<Store>,
    plan: &StartPlan,
    extra: &str,
) -> Result<String, IpcError> {
    const FROM: &str = "the tracker ticket's description";
    let s = lock(store)?;
    let item = plan
        .item_id
        .map(|id| s.get_work_item(id))
        .transpose()?
        .flatten();
    let meta = plan.item_id.map(|id| s.work_item_meta(id)).transpose()?;
    // The key is the tracker's text too (only the by-key fetch validates
    // its shape): flattened and defused like the title.
    let mut out = format!(
        "You are starting work on {}",
        tracker_line(&plan.key, KEY_LINE_MAX)
    );
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
    if !extra.trim().is_empty() {
        out.push_str(extra.trim_end());
        out.push('\n');
    }
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
    format!(
        "Start on {}: the ticket's context is in your fleet brief. Read it, then plan before \
         you edit.",
        tracker_line(key, KEY_LINE_MAX)
    )
}

/// Do the start. `spawn` makes the session (production: `new_session`);
/// `scope` is the caller's, for the refusal when another start won
/// meanwhile. Returns the new row, linked `started`, and whether a brief
/// was queued. A session that spawned but could not be linked is an error
/// here, naming that session as `orphan_session_id` so nobody has to find
/// it; a multi-repo start reports it as started with a warning
/// ([`start_one`]).
pub async fn start_with<F, Fut>(
    store: &Arc<Mutex<Store>>,
    plan: &StartPlan,
    brief: Option<String>,
    scope: &OrgScope,
    spawn: F,
) -> Result<(SessionRow, bool), IpcError>
where
    F: FnOnce(crate::service::sessions::NewSessionArgs) -> Fut,
    Fut: std::future::Future<Output = Result<SessionRow, IpcError>>,
{
    let one = start_one(store, plan, brief, scope, spawn).await?;
    match one.warning {
        Some(e) => {
            let mut d = e.details.clone().unwrap_or_else(|| serde_json::json!({}));
            if let Some(o) = d.as_object_mut() {
                o.insert("orphan_session_id".into(), one.row.id.into());
            }
            Err(e.with_details(d))
        }
        None => Ok((one.row, one.queued)),
    }
}

/// What [`start_one`] made: the row, whether its brief was queued, and the
/// error that followed a successful spawn (the link or the brief), if any.
#[derive(Debug)]
pub struct StartOutcome {
    pub row: SessionRow,
    pub queued: bool,
    pub warning: Option<IpcError>,
}

/// Spawn one session and link it `started`. `Err` only when the spawn
/// failed: once a session exists, a failure to link it (another start of
/// the same key won meanwhile, refused for `scope`) or to queue its brief
/// comes back as the outcome's `warning`, with the session.
pub async fn start_one<F, Fut>(
    store: &Arc<Mutex<Store>>,
    plan: &StartPlan,
    brief: Option<String>,
    scope: &OrgScope,
    spawn: F,
) -> Result<StartOutcome, IpcError>
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
    match link_started(store, plan, brief, scope, &row) {
        Ok((linked, queued)) => Ok(StartOutcome {
            row: linked.unwrap_or(row),
            queued,
            warning: None,
        }),
        Err(e) => Ok(StartOutcome {
            row,
            queued: false,
            warning: Some(e),
        }),
    }
}

/// Link a spawned session `started` and queue its brief. Returns the
/// re-read row and whether the brief was queued.
fn link_started(
    store: &Arc<Mutex<Store>>,
    plan: &StartPlan,
    brief: Option<String>,
    scope: &OrgScope,
    row: &SessionRow,
) -> Result<(Option<SessionRow>, bool), IpcError> {
    let s = lock(store)?;
    // The guard `plan_start` checked under is long gone (the spawn is an
    // SSH round trip): another start of the same key may have won since.
    // Re-check under the guard that writes the link.
    let item_org = plan.item_id.map(|id| s.item_org(id)).transpose()?.flatten();
    if let Some((_, other)) = live_work_on(&s, &plan.key, item_org)?
        .into_iter()
        .find(|(_, r)| {
            r.id != row.id && (!plan.per_project || r.project_id == Some(plan.project_id))
        })
    {
        // The same two-branch refusal as `plan_start`'s (D7): a winner the
        // caller may not see is not named.
        return Err(already_running(
            &plan.key,
            &other,
            scope,
            "",
            &format!(
                "the session this start made ({}) is not linked to it",
                row.tmux_name
            ),
        ));
    }
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
    Ok((s.get_session_by_id(row.id)?, queued))
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
    let (row, queued) = start_with(store, &plan, brief, scope, |a| {
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

// --- multi-repo start (work graph M9.6) ---------------------------------------

/// At most this many repositories in one start.
pub const MULTI_START_MAX: usize = 8;

/// The wall clock one multi-repo start may spend, under the MCP Lifecycle
/// cap (300 s) that bounds `work_link`'s work: past it the call is dropped
/// and its reply lost, so the starts stop short of it and report the rest.
pub const MULTI_START_BUDGET: std::time::Duration = std::time::Duration::from_secs(270);

/// A start is begun only with at least this much of the budget left; the
/// rest are reported `skipped` with reason [`SKIP_DEADLINE`].
pub const START_RESERVE: std::time::Duration = std::time::Duration::from_secs(45);

/// The `reason` of a repository skipped because the budget ran out.
pub const SKIP_DEADLINE: &str = "deadline";

/// Validate a multi-repo start's projects before anything else runs (the
/// operator's confirmation included): not with `project_id`, not empty, at
/// most [`MULTI_START_MAX`] distinct. Returns them deduplicated, in order.
pub fn multi_start_ids(project_id: Option<i64>, project_ids: &[i64]) -> Result<Vec<i64>, IpcError> {
    if project_id.is_some() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "pass project_id or project_ids, not both",
        ));
    }
    let mut ids: Vec<i64> = Vec::new();
    for id in project_ids {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    if ids.is_empty() || ids.len() > MULTI_START_MAX {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a multi-repo start takes 1 to {MULTI_START_MAX} project_ids"),
        ));
    }
    Ok(ids)
}

/// A repository a multi-repo start left alone: the key already runs there,
/// or (reason [`SKIP_DEADLINE`]) the time ran out before its turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSkip {
    pub project_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    pub reason: String,
}

/// A repository a multi-repo start could not start in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartFailure {
    pub project_id: i64,
    pub code: String,
    pub message: String,
    /// The refusal was the cross-org rule: `force_cross_org` would start it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cross_org: bool,
}

impl StartFailure {
    fn of(project_id: i64, e: &IpcError) -> Self {
        StartFailure {
            project_id,
            code: e.code.to_string(),
            message: e.message.to_string(),
            cross_org: e
                .details
                .as_ref()
                .is_some_and(|d| d["cross_org"] == serde_json::json!(true)),
        }
    }
}

/// A session a multi-repo start made that is not fully set up: it runs
/// (it is in `started`), but its link or brief failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWarning {
    pub project_id: i64,
    pub session_id: i64,
    pub code: String,
    pub message: String,
}

/// `work_link { action: start, project_ids }`: one sibling session per
/// repository, each on the same branch name (decision D11).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MultiStart {
    pub key: String,
    #[serde(default)]
    pub started: Vec<SessionRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<StartWarning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<StartSkip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<StartFailure>,
}

/// PURE: the line each sibling's brief carries about the others.
pub fn siblings_line(key: &str, here: &str, others: &[String], branch: &str) -> String {
    let key = tracker_line(key, KEY_LINE_MAX);
    let mut line = format!(
        "This is one of {} sessions starting {key} together, one per repository: this one \
         works in {here}",
        others.len() + 1
    );
    if !others.is_empty() {
        line.push_str(&format!("; the others in {}", others.join(", ")));
    }
    line.push_str(&format!(
        ". Each works on branch {branch} in its own repository; change only this one."
    ));
    line
}

/// `owner/repo on host` for each plan; `project N` when the store cannot
/// say (a label never fails the start).
fn sibling_labels(store: &Mutex<Store>, plans: &[StartPlan]) -> Vec<String> {
    let s = store.lock().ok();
    plans
        .iter()
        .map(|p| {
            let repo = s
                .as_ref()
                .and_then(|s| s.get_project(p.project_id).ok().flatten())
                .map(|r| format!("{}/{}", r.owner, r.repo))
                .unwrap_or_else(|| format!("project {}", p.project_id));
            format!("{repo} on {}", p.host_alias)
        })
        .collect()
}

/// The brief one sibling queues: the person's own, or the ticket's (only
/// when its org may reach the plan's host), with the siblings line.
fn sibling_brief(
    store: &Mutex<Store>,
    args: &StartArgs,
    plan: &StartPlan,
    sib: &str,
) -> Result<Option<String>, IpcError> {
    Ok(match (&args.brief, args.with_brief) {
        (Some(b), _) => Some(format!("{sib}\n\n{b}")),
        (None, true) if brief_visible_on(store, plan)? => {
            Some(ticket_brief_with(store, plan, sib)?)
        }
        (None, _) => None,
    })
}

/// Plan and start one sibling per project; `spawn` makes a session and
/// `started` runs right after each one whose brief was queued (production:
/// typing its start prompt), so a sibling never waits on the others.
///
/// The ticket is resolved once, so every repository gets one title and one
/// branch. A repo where the key (or a live session on the branch) already
/// runs is skipped, naming the session; one that fails to plan, brief or
/// spawn goes into `failed`; a session that spawned but failed to link is
/// `started` with a warning; and the rest still start. Starts run one at a
/// time (they share the host's SSH master), each begun only with
/// [`START_RESERVE`] left before `deadline`; the rest are skipped with
/// reason [`SKIP_DEADLINE`], and a start that outruns it is `E_TIMEOUT`.
#[allow(clippy::too_many_arguments)]
pub async fn start_many<F, Fut, P>(
    store: &Arc<Mutex<Store>>,
    args: &StartArgs,
    project_ids: &[i64],
    scope: &OrgScope,
    net: &TrackerNet,
    deadline: tokio::time::Instant,
    mut spawn: F,
    mut started: P,
) -> Result<MultiStart, IpcError>
where
    F: FnMut(crate::service::sessions::NewSessionArgs) -> Fut,
    Fut: std::future::Future<Output = Result<SessionRow, IpcError>>,
    P: FnMut(&SessionRow, &str),
{
    let ids = multi_start_ids(args.project_id, project_ids)?;
    let ticket = resolve_start(store, args, scope, net).await?;
    let mut out = MultiStart {
        key: ticket.key.clone(),
        ..Default::default()
    };
    let mut plans: Vec<StartPlan> = Vec::new();
    for pid in &ids {
        let one = StartArgs {
            project_id: Some(*pid),
            per_project: true,
            ..args.clone()
        };
        match plan_resolved(store, &one, scope, &ticket) {
            Ok(p) => plans.push(p),
            Err(e) if e.code == codes::E_EXISTS => out.skipped.push(StartSkip {
                project_id: *pid,
                session_id: e.details.as_ref().and_then(|d| d["session_id"].as_i64()),
                reason: e.message.to_string(),
            }),
            Err(e) => out.failed.push(StartFailure::of(*pid, &e)),
        }
    }
    let labels = sibling_labels(store, &plans);
    for (i, plan) in plans.iter().enumerate() {
        if deadline.saturating_duration_since(tokio::time::Instant::now()) < START_RESERVE {
            out.skipped.push(StartSkip {
                project_id: plan.project_id,
                session_id: None,
                reason: SKIP_DEADLINE.to_string(),
            });
            continue;
        }
        let others: Vec<String> = labels
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, l)| l.clone())
            .collect();
        let sib = siblings_line(&plan.key, &labels[i], &others, &plan.branch);
        let brief = match sibling_brief(store, args, plan, &sib) {
            Ok(b) => b,
            Err(e) => {
                out.failed.push(StartFailure::of(plan.project_id, &e));
                continue;
            }
        };
        let one =
            tokio::time::timeout_at(deadline, start_one(store, plan, brief, scope, &mut spawn))
                .await
                .unwrap_or_else(|_| {
                    Err(IpcError::new(
                        codes::E_TIMEOUT,
                        "the start outran the call's time; it may have partially completed",
                    ))
                });
        match one {
            Ok(one) => {
                if one.queued {
                    started(&one.row, &plan.key);
                }
                if let Some(e) = one.warning {
                    out.warnings.push(StartWarning {
                        project_id: plan.project_id,
                        session_id: one.row.id,
                        code: e.code.to_string(),
                        message: e.message.to_string(),
                    });
                }
                out.started.push(one.row);
            }
            Err(e) => out.failed.push(StartFailure::of(plan.project_id, &e)),
        }
    }
    Ok(out)
}

/// [`start_many`] over the real `new_session`, typing each sibling's start
/// prompt as soon as it is up, within [`MULTI_START_BUDGET`].
pub async fn start_work_many(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    args: &StartArgs,
    project_ids: &[i64],
    scope: &OrgScope,
    net: &TrackerNet,
) -> Result<MultiStart, IpcError> {
    start_many(
        store,
        args,
        project_ids,
        scope,
        net,
        tokio::time::Instant::now() + MULTI_START_BUDGET,
        |a| crate::service::sessions::new_session(a, store.as_ref(), ssh, reg),
        |row, key| {
            crate::service::work::resume::spawn_start_prompt(
                Arc::clone(store),
                Arc::clone(ssh),
                row,
                start_prompt(key),
            );
        },
    )
    .await
}

#[cfg(test)]
#[path = "tests_tickets.rs"]
mod tests;
