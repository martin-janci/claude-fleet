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
//!   `started` (`agent_started` when an agent starts it, D34), and — with a
//!   brief — queue the ticket's context (third-party text inside
//!   `mark_untrusted`) for the first hook.
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
use crate::service::work::resume::InFlight;
use crate::store::{Decider, SessionRow, Store, TrackerRow, WorkItemRow, WorkLinkRow, WorkTarget};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Default and maximum rows one `tickets` read returns.
pub const TICKETS_DEFAULT_LIMIT: usize = 50;
pub const TICKETS_MAX_LIMIT: usize = 200;
/// The `recent` view's window.
pub const RECENT_DAYS: i64 = 14;

/// Whether this item's tracker can serve a full description, and under which
/// key. A tracker fleet cannot identify, one with no key to name, or one
/// whose provider does not implement `describe`, points at the ticket
/// instead.
///
/// `pub(crate)`: all four callers that fence a description share this one
/// decision — `lookup` and `ticket_brief_with` here,
/// `service::work::card::card`, and `service::work::view::task`.
pub(crate) fn describe_offer<'a>(
    tracker: Option<&crate::store::TrackerRow>,
    key: Option<&'a str>,
) -> crate::mcp::guard::DescribeOffer<'a> {
    match (tracker, key) {
        (Some(t), Some(k)) if super::provider_caps(t).describe => {
            crate::mcp::guard::DescribeOffer::Key(k)
        }
        _ => crate::mcp::guard::DescribeOffer::None,
    }
}

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
    if let OrgScope::Org { .. } = scope {
        // A bound client (work graph M14): every item inside its orgs; no
        // host fence (a phone is not a host).
        return Ok(Some(
            s.work_item_orgs()?
                .into_iter()
                .filter(|(_, org)| scope.sees_org(*org))
                .map(|(id, _)| id)
                .collect(),
        ));
    }
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

/// May `scope` read `item`? Exactly `allowed(scope, s)` containing its id,
/// for the callers that ask about one item: without building the set of
/// every item the caller may see.
pub(crate) fn item_visible(
    scope: &OrgScope,
    s: &Store,
    item: &WorkItemRow,
) -> Result<bool, IpcError> {
    match scope {
        // This is the org boundary, not a privacy fence: which COMPANY's
        // work item this is. An item is not a session — its key and title
        // are item data — and every session field hung off it is
        // person-fenced where it is read (`tickets::live_ids` takes the
        // whole `ViewScope`).
        OrgScope::All => Ok(true),
        OrgScope::Org { .. } => Ok(scope.sees_org(s.item_org(item.id)?)),
        OrgScope::Host { alias, .. } => {
            Ok(s.work_item_on_host(alias, item.id)? && scope.sees_org(s.item_org(item.id)?))
        }
    }
}

/// The live sessions working on `key` as far as the item of `item_org` is
/// concerned. A link whose session is in one org while the item is in
/// another — a bare `ref_key` that `work_link` deliberately kept bare
/// because the key was outside the caller's orgs (work graph M5) — is not
/// live work on that item: a host of org A must not be able to block org
/// B's `start` or be named as working on B's ticket by typing its key. The
/// rule is [`Store::bind_tracker_refs`]'s: an unassigned side never
/// conflicts, and a forced item link keeps its item's org.
pub(crate) fn live_work_on(
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

/// The live sessions working on `item` that the reader may see (D7: an
/// isolated org's session is not named to a host outside it).
///
/// **The WHOLE [`ViewScope`], not its org half** (multi-user M1, T9c).
/// `Ticket.live_session_ids` is a list of session ids — spec §4.3 content,
/// and the same "somebody is working on this" bit `Graph::build`
/// person-fences one directory away — and the org half it used to be
/// filtered by (`OrgScope::sees_row_org_only`) is `true` for `OrgScope::All`,
/// i.e. for the master AND for every paired client bound to no org. So
/// `work { tickets }` enumerated the ids of every private live session on
/// each shared ticket to a second person's phone. T8's result gate cannot
/// net it either: the ids are a bare integer array, and
/// `view_scope::looks_like_session_row` needs an object with `host_alias`
/// and `tmux_name`.
///
/// [`ViewScope`]: crate::service::view_scope::ViewScope
fn live_ids(
    s: &Store,
    reader: &crate::service::view_scope::ViewScope,
    item: &WorkItemRow,
) -> Result<Vec<i64>, IpcError> {
    let Some(k) = item.key.as_deref() else {
        return Ok(Vec::new());
    };
    Ok(live_work_on(s, k, s.item_org(item.id)?)?
        .into_iter()
        // Org half; the next line, `sees_session_row`, is the person half.
        .filter(|(_, r)| reader.org.sees_row_org_only(r))
        .filter(|(_, r)| reader.sees_session_row(r).is_visible())
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
///
/// `include_local` (task → session P-4) adds the caller's own tasks — local
/// items a person made or accepted, never a dispatch's job mirror or a
/// proposal still waiting — after the tickets, under the same text filter
/// and limit, so "Work on task…" and ⌘K find them. Only for the unfiltered
/// list: a tracker or a tracker view names tickets alone.
pub fn tickets(
    store: &Mutex<Store>,
    tracker_id: Option<i64>,
    view: Option<&str>,
    query: Option<&str>,
    limit: Option<usize>,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<Vec<Ticket>, IpcError> {
    tickets_and_tasks(store, tracker_id, view, query, limit, false, reader)
}

/// [`tickets`], with the caller's own tasks after them when
/// `include_local` (task → session P-4).
#[allow(clippy::too_many_arguments)]
pub fn tickets_and_tasks(
    store: &Mutex<Store>,
    tracker_id: Option<i64>,
    view: Option<&str>,
    query: Option<&str>,
    limit: Option<usize>,
    include_local: bool,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<Vec<Ticket>, IpcError> {
    let want_local = include_local && tracker_id.is_none() && view.is_none();
    // The same gate as the task list (`work { local_items }`): a scoped
    // caller sees a task only through a link it can see, so a teammate's
    // private task is not listed here either. Read before the guard below,
    // which it takes itself.
    let visible: Option<HashSet<i64>> = if want_local {
        Some(
            crate::service::work::local::local_items(store, reader)?
                .into_iter()
                .map(|i| i.id)
                .collect(),
        )
    } else {
        None
    };
    let s = lock(store)?;
    let limit_n = limit
        .unwrap_or(TICKETS_DEFAULT_LIMIT)
        .clamp(1, TICKETS_MAX_LIMIT);
    let mut tasks: Vec<WorkItemRow> = Vec::new();
    if let Some(visible) = &visible {
        let allowed = allowed(&reader.org, &s)?;
        let q = query
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        for item in s.local_work_items()? {
            if tasks.len() >= limit_n {
                break;
            }
            if !visible.contains(&item.id)
                || allowed.as_ref().is_some_and(|a| !a.contains(&item.id))
            {
                continue;
            }
            let own = match item.origin.as_deref() {
                Some("agent") => false,
                Some("proposed") => item.proposal_state.as_deref() == Some("accepted"),
                _ => true,
            };
            if !own {
                continue;
            }
            if let Some(q) = &q {
                let hay = format!("{} {}", item.key.as_deref().unwrap_or_default(), item.title)
                    .to_lowercase();
                if !hay.contains(q.as_str()) {
                    continue;
                }
            }
            tasks.push(item);
        }
    }
    // Tickets first, but never the whole limit while tasks match: up to
    // half of it is kept for them, so a query matching many tickets still
    // shows the caller's TASK-n.
    let reserve = tasks.len().min(limit_n / 2);
    let mut out = tickets_in(&s, tracker_id, view, query, Some(limit_n - reserve), reader)?;
    for item in tasks {
        if out.len() >= limit_n {
            break;
        }
        let live = live_ids(&s, reader, &item)?;
        out.push(Ticket {
            item,
            live_session_ids: live,
            description: None,
            views: Vec::new(),
        });
    }
    Ok(out)
}

/// [`tickets`] under a store guard the caller already holds — the hook path
/// (the classification nudge, work graph M4.6) answers from inside one.
pub(crate) fn tickets_in(
    s: &Store,
    tracker_id: Option<i64>,
    view: Option<&str>,
    query: Option<&str>,
    limit: Option<usize>,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<Vec<Ticket>, IpcError> {
    let scope = &reader.org;
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
        let live = live_ids(s, reader, &item)?;
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
    if let OrgScope::Org { .. } = scope {
        // A bound client (work graph M14): the trackers of its orgs.
        return Ok(all
            .into_iter()
            .filter(|t| scope.sees_org(t.org_id))
            .collect());
    }
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
        // The configured trackers' hosts, so an enterprise GitHub URL is
        // recognised for exactly the instances fleet has (M11.4).
        let m = crate::service::work::recognize::recognize(
            r,
            &crate::service::work::recognize::RecognizeCtx {
                trackers: crate::service::work::detect::tracker_hosts(&trackers),
                ..Default::default()
            },
        )
        .into_iter()
        .find(|m| m.kind == crate::service::work::recognize::MatchKind::Url)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "not a ticket URL fleet recognises (Jira, GitHub, GitHub Enterprise, Asana or Linear)",
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
    reader: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<Ticket, IpcError> {
    let scope = &reader.org;
    let (tracker, key, by_url) = {
        let s = lock(store)?;
        match recognise(&s, reference) {
            Ok(r) => r,
            // Which sites are connected is not a host token's to learn: an
            // unknown site answers as an invisible key does.
            // This is the org boundary, not a privacy fence: which tracker SITES are connected
            // is an org question.
            Err(e) if e.code == codes::E_NOTFOUND && !scope.is_all() => {
                return Err(orgs::not_visible_to(scope, reference.trim()));
            }
            Err(e) => return Err(e),
        }
    };
    // A URL names its tracker: only that tracker's cache answers for it
    // (two sites can share a project key). A bare key asks every cache.
    let cached = match (&tracker, by_url) {
        (Some(t), true) => lock(store)?.tracker_item_for_key_in(t.id, &key)?,
        // A scoped caller's BARE key asks for the item IT may see: two
        // trackers (two sites, one per org) can hold the same key, which the
        // unscoped answer calls ambiguous and would refuse the caller over.
        // Not a URL whose tracker stayed open (the key's claimants are
        // several): that URL names one site's ticket, and another site's
        // item with the same key is a different ticket — it stays
        // ambiguous, as before.
        // This is the org boundary, not a privacy fence: which org's cache answers a bare key
        // when two sites hold the same one.
        (_, false) if !scope.is_all() => {
            let s = lock(store)?;
            let mut found = None;
            for item in s.tracker_items_for_key(&key)? {
                if item_visible(scope, &s, &item)? {
                    found = Some(item);
                    break;
                }
            }
            found
        }
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
            // A bound client (M14) may make the hub fetch only from a tracker
            // of its own orgs.
            // This is the org boundary, not a privacy fence: which tracker this caller may make
            // the hub fetch from.
            if !scope.is_all() && !tracker.as_ref().is_some_and(|t| scope.sees_org(t.org_id)) {
                return Err(orgs::not_visible_to(scope, &key));
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
    let item = s
        .get_work_item(item_id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "item vanished"))?;
    if !item_visible(scope, &s, &item)? {
        return Err(orgs::not_visible_to(scope, &key));
    }
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
    let live = live_ids(&s, reader, &item)?;
    // The key may end up in DescribeOffer::Key, which fence_ticket puts in
    // fleet's own notice line: flatten it like every other tracker line does
    // (defuse alone does not fold a newline).
    let flat_key = item
        .key
        .as_deref()
        .map(|k| k.split_whitespace().collect::<Vec<_>>().join(" "));
    // An agent reads it: fenced on both sides, markers defused (M3 review),
    // with a trailing notice when the cache kept less than the tracker holds
    // (Task 1's 2k cap).
    let description = meta.description.map(|d| match scope {
        OrgScope::Host { .. } => crate::mcp::guard::fence_ticket(
            &d,
            "a tracker ticket",
            super::DESCRIPTION_MAX_CHARS,
            meta.description_chars,
            describe_offer(t.as_ref(), flat_key.as_deref()),
        ),
        // A person reads it on a phone or the desktop (bound or not): as is.
        // This is the org boundary, not a privacy fence: what is trimmed for
        // a per-host token is a TICKET's description — the tracker's own
        // text, which belongs to a company and names no session.
        OrgScope::All | OrgScope::Org { .. } => d,
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
    /// A second session on a key that already has a live one (task → session
    /// spec P-8): the key's duplicate guard is skipped, never the branch's,
    /// and an unnamed worktree is a fresh `<slug>-N`, so two Claudes never
    /// share a checkout.
    pub parallel: bool,
    /// Who starts (D34): a person's start links `started`, an agent's
    /// (a per-host token, the operator) `agent_started`. The desktop's is
    /// always a person's (the default).
    pub decider: Decider,
    /// WHOSE the started session is (multi-user M1, T5): the `people` row of
    /// the caller who asked for the start, `None` for a caller that is no
    /// person (a per-host token, the operator) or for a hub that cannot say.
    ///
    /// Never on the wire — these args are built from a tool's own parameters
    /// and this field is filled from `Caller`, never from JSON — because a
    /// value a caller could set would be a way to start a session in somebody
    /// else's name.
    pub owner: Option<i64>,
    /// Who or what is starting it (migration 124): a mission's run, an
    /// agent's token, the operator. `None` = a person, recorded with
    /// `owner`. Never on the wire, for `owner`'s reason.
    pub origin: Option<crate::store::SessionOrigin>,
    /// The login the session bills (redesign 8.7): a credential profile on
    /// the host; `None` = the host's own. Set by a mission's grant.
    pub profile: Option<String>,
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
    /// A parallel start ([`StartArgs::parallel`]): the key's duplicate guard
    /// is not this plan's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parallel: bool,
    /// Who starts ([`StartArgs::decider`]): decides the link's source.
    /// Never on the wire: a plan read back is a person's.
    #[serde(skip)]
    pub decider: Decider,
    /// Whose the started session is ([`StartArgs::owner`]). `#[serde(skip)]`
    /// for the same reason `decider` is: a plan a client hands back must not
    /// be able to name an owner.
    #[serde(skip)]
    pub owner: Option<i64>,
    /// Who or what starts it ([`StartArgs::origin`]). `#[serde(skip)]` for
    /// `owner`'s reason: a plan a client hands back must not name a mission.
    #[serde(skip)]
    pub origin: Option<crate::store::SessionOrigin>,
    /// The start rule (redesign 8.11) that picked the project, when one
    /// did: the popover says "by rule PD-*".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// The login it bills: a mission's grant ([`StartArgs::profile`]), else
    /// the start rule's account, else the placement rule's (gap plan G7.1).
    /// Shown on the preview; never read back from a client.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The start rule's model, effort and agent (G7.1). Shown on the
    /// preview; never read back from a client.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The rule's own host, when it was unreachable and the start took the
    /// rule's fallback host instead ("mercury, mac is offline").
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub fell_back_from: Option<String>,
    /// The placement rule whose "its sessions start here" named the host or
    /// the account (G7.1).
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub placement_rule_id: Option<i64>,
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
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<StartPlan, IpcError> {
    let ticket = resolve_start(store, args, view, net).await?;
    plan_resolved(store, args, view, &ticket)
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
    reader: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<StartTicket, IpcError> {
    resolve_ticket(store, args, reader, net, false).await
}

/// [`resolve_start`]; `allow_proposal` lets an unaccepted proposal through
/// for a preview, which reports it as a conflict instead.
async fn resolve_ticket(
    store: &Mutex<Store>,
    args: &StartArgs,
    reader: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
    allow_proposal: bool,
) -> Result<StartTicket, IpcError> {
    let scope = &reader.org;
    let (key, title, item_id) = match (args.item_id, args.reference.as_deref()) {
        (Some(id), None) => {
            let s = lock(store)?;
            // Out of scope reads exactly as unknown: no existence oracle.
            let item = match s.get_work_item(id)? {
                Some(item) if item_visible(scope, &s, &item)? => item,
                _ => return Err(orgs::not_found("work item", id)),
            };
            if !allow_proposal
                && item.origin.as_deref() == Some("proposed")
                && item.proposal_state.as_deref() != Some("accepted")
            {
                return Err(IpcError::new(codes::E_INVALID, "accept the proposal first"));
            }
            let key = item.key.clone().ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "that work item has no key to start from")
            })?;
            (key, item.title, Some(id))
        }
        (None, Some(r)) => match lookup(store, r, reader, net).await {
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

/// `E_EXISTS` naming `row` (or not, when the caller cannot see it — an
/// isolated org's session is not named to a host outside it, D7; another
/// PERSON's session is not named to anybody, multi-user M1 T9b). `what`
/// qualifies the session ("on branch x"); `then` is what to do about it.
///
/// **The fence is [`crate::service::view_scope::ViewScope`], not its org
/// half.** The long form prints the occupant's friendly-or-tmux name and its
/// host, and `OrgScope::All` is what every paired client bound to no org
/// resolves to — so `work_link { start, key: PAY-123 }` answered with another
/// person's session name and machine. The `details` object IS netted by T8's
/// result gate (`session_id` + `host_alias` + `tmux_name` → a row shape), but
/// the TEXT block is not: `support::rewrite_json_content` skips any content
/// block that will not parse as JSON. The sibling path was hardened against
/// exactly this (`repo.rs`'s `E_WORKTREE_BUSY` prints a COUNT, never a
/// host or a tmux name); this one was missed.
///
/// An invisible occupant is then indistinguishable from a merely-busy key,
/// which is the point: the bare sentence is no oracle either way.
fn already_running(
    key: &str,
    row: &SessionRow,
    view: &crate::service::view_scope::ViewScope,
    what: &str,
    then: &str,
) -> IpcError {
    if !view.sees_session_row(row).is_visible() {
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

/// Where a start of `key` would land before any host is named: a start rule
/// (redesign 8.11) first when the caller named no project (`project_id`
/// `None`), before the key's history and so before Jev is ever asked; else
/// where this kind of work last ran, a GitHub issue's own repository's
/// project first, else the newest link with the same key prefix. The
/// rule that decided, the `(project, host)` (host `""` when unknown) and the
/// label an "ambiguous" refusal names. Reads only: the mission loop's
/// over-limit hold asks it too, so it checks the host the start lands on.
#[allow(clippy::type_complexity)]
pub(crate) fn seen_place(
    s: &Store,
    key: &str,
    item_org: Option<i64>,
    project_id: Option<i64>,
) -> Result<
    (
        Option<crate::store::StartRuleRow>,
        Option<(i64, String)>,
        String,
    ),
    IpcError,
> {
    let rule = match project_id {
        None => crate::service::start_rules::matching(s, item_org, key)?,
        Some(_) => None,
    };
    let (seen, label) = match crate::store::github_ref(key) {
        _ if rule.is_some() => {
            let r = rule.as_ref().expect("checked above");
            let last = s.last_host_for_project(r.project_id)?;
            let (host, _) = crate::service::start_rules::rule_host(s, r, last.as_deref())?;
            (
                Some((r.project_id, host.unwrap_or_default())),
                String::new(),
            )
        }
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
    Ok((rule, seen, label))
}

/// The host a start in `project_id` lands on when none is named: the
/// [`seen_place`] host when it is this project's, else the host the project
/// last ran on.
pub(crate) fn seen_host(
    s: &Store,
    seen: Option<(i64, String)>,
    project_id: i64,
) -> Result<Option<String>, IpcError> {
    match seen
        .filter(|p| p.0 == project_id && !p.1.is_empty())
        .map(|p| p.1)
    {
        Some(h) => Ok(Some(h)),
        None => Ok(s.last_host_for_project(project_id)?),
    }
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
    view: &crate::service::view_scope::ViewScope,
    ticket: &StartTicket,
) -> Result<StartPlan, IpcError> {
    let scope = &view.org;
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
    let live = if args.parallel {
        Vec::new()
    } else {
        live_work_on(&s, &key, item_org)?
    };
    if let Some((_, row)) = live
        .iter()
        .find(|(_, r)| !args.per_project || r.project_id == args.project_id)
    {
        return Err(already_running(&key, row, view, "", "jump to it"));
    }
    let (rule, seen, prefix_label) = seen_place(&s, &key, item_org, args.project_id)?;
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
            .with_details(serde_json::json!({ "missing": "project", "candidates": candidates })));
        }
    };
    // How the start runs (gap plan G7.1): the start rule's account, model,
    // effort and agent; with no start rule naming a host, the placement
    // rule's "its sessions start here" host and account.
    let rule_names_host = seen
        .as_ref()
        .is_some_and(|p| rule.is_some() && p.0 == project_id && !p.1.is_empty());
    let placement = if rule_names_host && rule.as_ref().is_some_and(|r| r.profile.is_some()) {
        None
    } else {
        let repo = s
            .get_project(project_id)?
            .map(|p| format!("{}/{}", p.owner, p.repo));
        crate::service::start_rules::placement_start(&s, item_id, &key, &title, repo.as_deref())?
    };
    let fell_back_from = match (&rule, &seen) {
        (Some(r), Some(p)) if p.0 == project_id && r.fallback_host.as_deref() == Some(&p.1) => r
            .host_alias
            .clone()
            .or(s.last_host_for_project(project_id)?)
            .filter(|h| h != &p.1),
        _ => None,
    };
    let host_alias = match (&args.host_alias, scope.host()) {
        (Some(h), _) => h.clone(),
        (None, Some(h)) => h.to_string(),
        (None, None)
            if !rule_names_host && placement.as_ref().is_some_and(|w| w.host_alias.is_some()) =>
        {
            placement
                .as_ref()
                .and_then(|w| w.host_alias.clone())
                .expect("checked above")
        }
        (None, None) => match seen_host(&s, seen, project_id)? {
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
                .with_details(serde_json::json!({ "missing": "host", "candidates": hosts })));
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
    // A client bound to an org (M14) starts work only where the session
    // would be its org's: never a session it could not see once made.
    // With D31 off, an unassigned session is not its to see either.
    if scope.bound_org().is_some() {
        let session_org = s.org_for_new_session(&host_alias, project_id)?;
        if !scope.sees_org(session_org) {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a client bound to an org starts work only in its own org's projects and hosts",
            ));
        }
    }
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
    let checkouts: Vec<_> = s
        .list_worktrees_on_host(&host_alias)?
        .into_iter()
        .filter(|w| w.project_id == project_id)
        .collect();
    let branch = if args.parallel && args.worktree.is_none() {
        free_branch(&branch, |b| checkouts.iter().any(|w| w.name == b))
    } else {
        branch
    };
    let worktree_id = checkouts.iter().find(|w| w.name == branch).map(|w| w.id);
    // A start whose branch slug matches an EXISTING checkout lands its pane
    // in that checkout — so `work_link { start }` acts on an existing row
    // after all, and takes the same landing gate `new_session` /
    // `new_shell_session` take (multi-user M1, T9b; the hole T8d's blocker 5
    // closed, reopened one arm over). `new_worktree` is untouched: a tree
    // that does not exist yet has no occupants, and `worktree_id` is `None`
    // on that path.
    crate::service::sessions::require_may_land_in_worktree(&s, view, worktree_id)?;
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
                view,
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
        parallel: args.parallel,
        decider: args.decider,
        owner: args.owner,
        origin: args.origin.clone(),
        profile: args
            .profile
            .clone()
            .or_else(|| rule.as_ref().and_then(|r| r.profile.clone()))
            .or_else(|| {
                let codex = rule.as_ref().and_then(|r| r.agent.as_deref())
                    == Some(crate::store::AGENT_CODEX);
                placement
                    .as_ref()
                    .and_then(|w| w.profile.clone())
                    .filter(|_| !codex)
            }),
        model: rule.as_ref().and_then(|r| r.model.clone()),
        effort: rule.as_ref().and_then(|r| r.effort.clone()),
        agent: rule.as_ref().and_then(|r| r.agent.clone()),
        fell_back_from,
        placement_rule_id: placement.as_ref().map(|w| w.id),
        rule_id: rule.map(|r| r.id),
    })
}

/// `base`, else the first `base-N` (N ≥ 2) that `taken` does not hold: a
/// parallel start's own checkout.
fn free_branch(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|b| !taken(b))
        .expect("an unbounded range has a free name")
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
    // The tracker that might serve a full description, mirroring `lookup`'s
    // resolution: this function has no `t` of its own, only the item.
    let tracker = s
        .list_trackers()?
        .into_iter()
        .find(|t| Some(t.id) == item.as_ref().and_then(|i| i.tracker_id));
    // The key may end up in DescribeOffer::Key, which fence_ticket puts in
    // fleet's own notice line: flatten it like every other tracker line does
    // (defuse alone does not fold a newline).
    let flat_key = item
        .as_ref()
        .and_then(|i| i.key.as_deref())
        .map(|k| k.split_whitespace().collect::<Vec<_>>().join(" "));
    if let Some(m) = meta {
        if let Some(d) = m.description.clone() {
            let overhead = out.chars().count()
                + crate::mcp::guard::fence_untrusted("", FROM, 0)
                    .chars()
                    .count()
                + 2;
            let max_total = crate::service::work::handover::BRIEF_MAX_CHARS;
            let budget = max_total.saturating_sub(overhead);
            // budget 0 no longer means silence: fence_ticket says it did not
            // fit. The notice it may append sits outside `budget`, so a cut
            // that fills the whole budget can push the total past
            // `max_total` by the notice's own length; when it does, shrink
            // the fence by exactly that much and let fence_ticket re-report
            // a smaller `shown` (the doc comment's "always fits" promise).
            //
            // Computing the notice's length up front instead, to size
            // `budget` correctly the first time, would be circular: the
            // notice names `shown`, and `shown` *is* the budget once the
            // text is actually being truncated — so the budget would depend
            // on the notice's own length, which depends on the budget. One
            // fixpoint iteration (build once, measure the real overflow,
            // shrink, build once more) is the correct shape here. Do not
            // "simplify" this back into a circle.
            let mut fenced = crate::mcp::guard::fence_ticket(
                &d,
                FROM,
                budget,
                m.description_chars,
                describe_offer(tracker.as_ref(), flat_key.as_deref()),
            );
            // The real total once pushed below is `out` as it stands now,
            // plus the two newlines, plus `fenced` (which already carries
            // its own fence overhead) — NOT `overhead + fenced.len()`:
            // `overhead` already counts an (empty) fence's overhead once, so
            // adding `fenced.len()` on top would count it twice and over-cut
            // by the fence's own fixed overhead (~130 chars).
            let total = out.chars().count() + 2 + fenced.chars().count();
            let over = total.saturating_sub(max_total);
            if over > 0 {
                // `shown` is the number of characters actually inside the
                // fence — `min(budget, len(text))` — not `budget` itself. A
                // stored description is capped at `DESCRIPTION_MAX_CHARS`
                // (Task 1), so it is usually *shorter* than `budget`
                // (content-bound, not budget-bound): shrinking `budget` by
                // `over` then only starts reducing `shown` once the shrunk
                // budget drops below the text's length, wasting up to
                // `budget - shown` of the shrink and leaving the brief still
                // over by that much (this is exactly the failure a fix
                // review caught: a band of `extra` lengths that stayed
                // 1–76 chars over). `defuse` is a same-length substitution,
                // so it never changes the character count: `shown` here
                // matches exactly what `fence_ticket` computed internally.
                // Shrinking `shown` itself, not `budget`, always lands on or
                // under the limit in one step.
                let shown = d.chars().count().min(budget);
                let budget = shown.saturating_sub(over);
                fenced = crate::mcp::guard::fence_ticket(
                    &d,
                    FROM,
                    budget,
                    m.description_chars,
                    describe_offer(tracker.as_ref(), flat_key.as_deref()),
                );
            }
            out.push('\n');
            out.push_str(&fenced);
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
    view: &crate::service::view_scope::ViewScope,
    spawn: F,
) -> Result<(SessionRow, bool), IpcError>
where
    F: FnOnce(crate::service::sessions::NewSessionArgs) -> Fut,
    Fut: std::future::Future<Output = Result<SessionRow, IpcError>>,
{
    // Work graph M14: hold the key in the registry `resume` uses from
    // before the spawn until the link is written (dropped on every exit),
    // so a second device's start of the same ticket, or a resume of it, is
    // refused here instead of spawning a session that loses the race.
    let _claim = {
        let s = lock(store)?;
        let claim = InFlight::claim(&s, &plan.key)?;
        // `plan_start`'s guard ran before the claim: a start that claimed,
        // linked and released since is caught here, before the spawn.
        if let Some(other) = rival(&s, plan, None)? {
            return Err(already_running(&plan.key, &other, view, "", "jump to it"));
        }
        claim
    };
    let one = start_one(store, plan, brief, view, spawn).await?;
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
    view: &crate::service::view_scope::ViewScope,
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
        model: plan.model.clone(),
        effort: plan.effort.clone(),
        profile: plan.profile.clone(),
        agent: plan.agent.clone(),
        // Who or what started it (migration 124); `None` = a person.
        origin: plan.origin.clone(),
        // Whose the started session is (multi-user M1, T5): the caller who
        // asked for the start (`StartArgs::owner`, filled from `Caller` at
        // the tool), and the hub's own person only for a path that genuinely
        // has no caller — the operator's own starts, the catalog's author
        // session, the desktop. Without the first half, a second person's
        // `work_link { start }` created a session owned by the HUB's owner:
        // readable by somebody who did not ask for it, and `null` in the
        // answer to the person who did (T8 drops the row they may not see).
        over_limit_ok: false,
        owner_person_id: plan
            .owner
            .or_else(|| crate::service::sessions::hub_personal_owner(store)),
        start_token: None,
    };
    let row = spawn(args).await?;
    let outcome = match link_started(store, plan, brief, view, &row) {
        Ok((linked, queued)) => StartOutcome {
            row: linked.unwrap_or(row),
            queued,
            warning: None,
        },
        Err(e) => StartOutcome {
            row,
            queued: false,
            warning: Some(e),
        },
    };
    record_start_steps(store, plan, &outcome.row, outcome.queued);
    Ok(outcome)
}

/// The timeline kind for a session a start has just made (task → session
/// P-5). Its detail is a [`StartSpawned`]: what `abandon_start` (P-6)
/// reads to know the checkout is the start's own.
pub const START_SPAWNED: &str = "start_spawned";
/// The start's checkout is in place (P-5); its detail is the branch.
pub const WORKTREE_READY: &str = "worktree_ready";

/// The detail of a [`START_SPAWNED`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSpawned {
    pub key: String,
    /// The session's checkout, when it has one.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// Whether this start created that checkout (and its branch), rather
    /// than reusing one.
    #[serde(default)]
    pub new_worktree: bool,
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether a brief was queued, so the start prompt will be typed and
    /// `repl_ready` / `handover_started` will follow; without one the
    /// start's progress ends here.
    #[serde(default)]
    pub brief: bool,
}

/// Write a fresh start's first progress steps on its timeline: the session
/// exists (`start_spawned`) in its checkout (`worktree_ready`). Best-effort,
/// like every timeline write: a start never fails over its progress strip.
fn record_start_steps(store: &Mutex<Store>, plan: &StartPlan, row: &SessionRow, brief: bool) {
    let new_worktree = plan.worktree_id.is_none() && row.worktree_id.is_some();
    let detail = StartSpawned {
        key: plan.key.clone(),
        worktree_id: row.worktree_id,
        new_worktree,
        branch: row.worktree_id.map(|_| plan.branch.clone()),
        brief,
    };
    let Ok(s) = store.lock() else {
        return;
    };
    let json = serde_json::to_string(&detail).unwrap_or_default();
    let _ = s.insert_session_event(row.id, START_SPAWNED, Some(&json));
    if row.worktree_id.is_some() {
        let _ = s.insert_session_event(row.id, WORKTREE_READY, Some(&plan.branch));
    }
}

/// A live session of `plan`'s key other than `except` (in `plan`'s project
/// for a multi-repo sibling): the start that would make a second one.
fn rival(s: &Store, plan: &StartPlan, except: Option<i64>) -> Result<Option<SessionRow>, IpcError> {
    if plan.parallel {
        return Ok(None);
    }
    let item_org = plan.item_id.map(|id| s.item_org(id)).transpose()?.flatten();
    Ok(live_work_on(s, &plan.key, item_org)?
        .into_iter()
        .map(|(_, r)| r)
        .find(|r| {
            Some(r.id) != except && (!plan.per_project || r.project_id == Some(plan.project_id))
        }))
}

/// Link a spawned session `started` and queue its brief. Returns the
/// re-read row and whether the brief was queued.
fn link_started(
    store: &Arc<Mutex<Store>>,
    plan: &StartPlan,
    brief: Option<String>,
    view: &crate::service::view_scope::ViewScope,
    row: &SessionRow,
) -> Result<(Option<SessionRow>, bool), IpcError> {
    let s = lock(store)?;
    // The guard `plan_start` checked under is long gone (the spawn is an
    // SSH round trip): the claim fences other starts and resumes, but not a
    // person's own link of the key meanwhile. Re-check under the guard that
    // writes the link.
    if let Some(other) = rival(&s, plan, Some(row.id))? {
        // The same two-branch refusal as `plan_start`'s (D7): a winner the
        // caller may not see is not named.
        return Err(already_running(
            &plan.key,
            &other,
            view,
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
    // `started` for a person, `agent_started` for an agent (D34).
    s.link_session_work(row.id, target, plan.decider.start_source())?;
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

/// A native item (design 2026-09-29) starts where it belongs and with what
/// was written: an unset project comes from the item, else its parent's;
/// an unset brief is the parent's (a ticket's own description, fenced as
/// every start brief is) followed by the item's title and notes. Anything
/// else starts exactly as asked.
fn with_native_defaults(
    store: &Mutex<Store>,
    args: &StartArgs,
    scope: &OrgScope,
) -> Result<StartArgs, IpcError> {
    let Some(id) = args.item_id else {
        return Ok(args.clone());
    };
    let s = lock(store)?;
    let Some(item) = s.get_work_item(id)? else {
        return Ok(args.clone());
    };
    if !matches!(
        item.origin.as_deref(),
        Some("manual" | "proposed" | "agent")
    ) {
        return Ok(args.clone());
    }
    // The parent's title, brief and project reach the new session only when
    // this caller may see the parent (another org's ticket text must not
    // ride a subtask's start brief across the org fence).
    let parent = match item
        .parent_id
        .map(|p| s.get_work_item(p))
        .transpose()?
        .flatten()
    {
        Some(p) if item_visible(scope, &s, &p)? => Some(p),
        _ => None,
    };
    let mut out = args.clone();
    if out.project_id.is_none() {
        out.project_id = item
            .project_id
            .or_else(|| parent.as_ref().and_then(|p| p.project_id));
    }
    if out.brief.is_none() {
        let own = match item.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            Some(n) => format!("{}\n\n{n}", item.title),
            None => item.title.clone(),
        };
        out.brief = Some(match &parent {
            Some(p) => {
                let head = match p.key.as_deref() {
                    Some(k) => format!("{k} {}", p.title),
                    None => p.title.clone(),
                };
                let desc = s
                    .work_item_meta(p.id)?
                    .description
                    .filter(|d| !d.trim().is_empty())
                    .map(|d| {
                        crate::mcp::guard::fence_untrusted(
                            &d,
                            "a tracker ticket",
                            crate::service::work::view::DESCRIPTION_MAX_CHARS,
                        )
                    });
                let task = match desc {
                    Some(d) => format!("## Task {head}\n\n{d}"),
                    None => format!("## Task {head}"),
                };
                let brief = format!(
                    "{task}\n\n## Subtask {}\n\n{own}",
                    item.key.as_deref().unwrap_or_default()
                );
                brief
                    .chars()
                    .take(crate::service::work::handover::BRIEF_MAX_CHARS)
                    .collect()
            }
            None => own,
        });
    }
    Ok(out)
}

/// `work_link { action: start }` end to end over the real `new_session`.
pub async fn start_work(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    args: &StartArgs,
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<SessionRow, IpcError> {
    let (row, plan, queued) = start_work_unprompted(store, ssh, reg, args, view, net).await?;
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

/// [`start_work`] without its first prompt: the session, linked, with its
/// brief queued (`true` when one was), and the plan it was started on. The
/// caller types its own prompt, as `work_link { run }` does to append its
/// task's done-marker instruction.
pub async fn start_work_unprompted(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    args: &StartArgs,
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<(SessionRow, StartPlan, bool), IpcError> {
    // Main's native-subtask defaults, under M1's `ViewScope`: the step needs
    // only the org half, which is what `with_native_defaults` takes.
    let args = &with_native_defaults(store, args, &view.org)?;
    let plan = plan_start(store, args, view, net).await?;
    let brief = match (&args.brief, args.with_brief) {
        (Some(b), _) => Some(b.clone()),
        // The brief is read by the new session's Claude: never another
        // org's ticket text, even on a forced cross-org start.
        (None, true) if brief_visible_on(store, &plan)? => Some(ticket_brief(store, &plan)?),
        (None, _) => None,
    };
    let (row, queued) = start_with(store, &plan, brief, view, |a| {
        crate::service::sessions::new_session(a, store.as_ref(), ssh, reg)
    })
    .await?;
    // Redesign 8.11: a rule that decided counts the start; a person's start
    // no rule decided counts toward offering one.
    match plan.rule_id {
        Some(id) => crate::service::start_rules::note_hit(store, id),
        None if args.decider == Decider::Person => crate::service::start_rules::tally_logged(
            store,
            plan.item_id,
            &plan.key,
            plan.project_id,
        ),
        None => {}
    }
    // Jev K1's follow-up: where a person's start landed answers a proposal
    // they were shown. Best effort; an agent's start answers nothing.
    if args.decider == Decider::Person {
        if let Ok(s) = lock(store) {
            if let Err(e) = crate::service::decide::start_project::record_start(
                &s,
                plan.item_id,
                &plan.key,
                plan.project_id,
                crate::store::now_unix(),
            ) {
                tracing::warn!(
                    "[decide] start_project follow-up not recorded: {}",
                    e.message
                );
            }
        }
        // Jev N3's: a single start ticked no sibling.
        record_sibling_start(
            store,
            plan.item_id,
            &plan.key,
            plan.project_id,
            &[plan.project_id],
        );
    }
    Ok((row, plan, queued))
}

// --- start preview (task → session spec P-1) ---------------------------------

/// What would stop or change a start, as data rather than an error: the
/// Work button's popover shows each with its choices before anything is
/// made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartConflict {
    /// `live_session` | `proposal` | `cross_org` | `worktree_busy` | `done`.
    pub kind: String,
    /// One plain sentence for the person.
    pub message: String,
    /// The session in the way, only when the caller may see it (D7, T9b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
}

/// A project a start could land in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectChoice {
    pub id: i64,
    pub owner: String,
    pub repo: String,
}

/// A host a start could land on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostChoice {
    pub alias: String,
    pub reachable: bool,
}

/// The checkout a planned start would use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutPreview {
    /// The planned worktree already exists and is reused.
    pub exists: bool,
    /// A live session is working in it, when the caller may see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_by: Option<i64>,
}

/// `work_link { action: start, dry_run: true }`: where a start would land,
/// what it would send, and what is in the way — nothing is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPreview {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub item_id: Option<i64>,
    /// The resolved start; `None` while `missing` names what to pick.
    #[serde(default)]
    pub plan: Option<StartPlan>,
    /// `project` | `host`: what the person must pick before a start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    pub projects: Vec<ProjectChoice>,
    pub hosts: Vec<HostChoice>,
    pub conflicts: Vec<StartConflict>,
    /// The brief the start would queue, as it stands.
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub checkout: Option<CheckoutPreview>,
    /// With `missing: "project"`: the candidate the decision model proposes
    /// (Jev K1, `decide.jev.start_project` at `assist`). A pre-selection
    /// only; the person still starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_project: Option<crate::service::decide::start_project::SuggestedProject>,
    /// The same pre-selection in the shape every row proposes in (step 2.8):
    /// feature `start_project`, value `p<id>`. `suggested_project`
    /// stays for clients that read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<crate::store::DecisionProposal>,
    /// With a planned project: the sibling repository (one of the projects
    /// the key ran in before) the decision model proposes the same task
    /// also needs (Jev N3, `decide.jev.sibling_repos` at `assist`). A
    /// pre-tick only; the person still starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_sibling: Option<crate::service::decide::sibling_repos::SuggestedSibling>,
    /// The start rule fleet offers for this key after a person started its
    /// prefix in one project five times in a row (redesign 8.11): "Add rule
    /// PD-* → papaya-pos?". Accept or dismiss it with `start_rules`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_offer: Option<crate::store::StartRuleRow>,
    /// The brief above was drafted by a model (redesign 6.10, `draft_brief`):
    /// by which, on which host, from how many notes. `None` for the template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_draft: Option<crate::service::work::brief_draft::BriefDraft>,
}

/// The projects `key` ran in before, other than `chosen`: ended links'
/// `snap_project_id` and live sessions linked to the key (under the item's
/// org, as [`live_work_on`] fences them). Newest first, limited to projects
/// the fleet still has, system projects left out. The backend's mirror of
/// the dialog's `siblingCandidates` (`src/lib/multi_start.ts`).
pub fn sibling_candidates(
    s: &Store,
    key: &str,
    item_org: Option<i64>,
    chosen: i64,
) -> Result<Vec<crate::service::decide::start_project::Candidate>, IpcError> {
    let mut seen: Vec<(i64, i64)> = Vec::new();
    let mut add = |id: Option<i64>, at: i64| {
        let Some(id) = id.filter(|&id| id != chosen) else {
            return;
        };
        match seen.iter_mut().find(|(p, _)| *p == id) {
            Some(had) => had.1 = had.1.max(at),
            None => seen.push((id, at)),
        }
    };
    for l in s.ended_work_links_for_key(key)? {
        add(l.snap_project_id, l.ended_at.unwrap_or(l.created_at));
    }
    for (_, row) in live_work_on(s, key, item_org)? {
        add(row.project_id, row.last_activity_at);
    }
    // Stable: equal times keep their first-seen order, as the dialog's do.
    seen.sort_by_key(|&(_, at)| std::cmp::Reverse(at));
    let mut out = Vec::new();
    for (id, _) in seen {
        if let Some(p) = s.get_project(id)? {
            if !p.system {
                out.push(crate::service::decide::start_project::Candidate {
                    project_id: p.id,
                    owner: p.owner,
                    repo: p.repo,
                });
            }
        }
    }
    Ok(out)
}

/// [`preview_start`], and when it answers `missing: "project"`, the
/// decision model's pre-selection (Jev K1, `service::decide::start_project`):
/// in `shadow` asked off this path and only recorded; in `assist` awaited
/// (one call, bounded by `decide.jev.timeout_ms`) and returned as
/// `suggested_project`. With the defaults the gate refuses and nothing is
/// asked. `decide` is `None` where no call may be made.
///
/// When the preview has a planned project instead, the same applies to the
/// sibling repository the task also needs (Jev N3,
/// `service::decide::sibling_repos`), asked over [`sibling_candidates`] and
/// returned as `suggested_sibling`.
pub async fn preview_start_decided(
    store: &Mutex<Store>,
    args: &StartArgs,
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
    decide: Option<&crate::service::decide::DecideCtx>,
) -> Result<StartPreview, IpcError> {
    use crate::service::decide::{start_project, Mode};
    let mut preview = preview_start(store, args, view, net).await?;
    let Some(ctx) = decide else {
        return Ok(preview);
    };
    if preview.missing.is_none() {
        if let Some(plan) = preview.plan.clone() {
            suggest_sibling(store, ctx, &mut preview, &plan).await?;
        }
        return Ok(preview);
    }
    if preview.missing.as_deref() != Some("project") {
        return Ok(preview);
    }
    let input = {
        let s = lock(store)?;
        let org_id = preview
            .item_id
            .map(|id| s.item_org(id))
            .transpose()?
            .flatten();
        let description = preview
            .item_id
            .map(|id| s.work_item_meta(id))
            .transpose()?
            .and_then(|m| m.description);
        start_project::StartInput {
            key: preview.key.clone(),
            title: preview.title.clone(),
            item_id: preview.item_id,
            org_id,
            description,
            candidates: start_project::fenced(
                &s,
                crate::service::decide::Feature::StartProject,
                org_id,
                &preview
                    .projects
                    .iter()
                    .map(|p| start_project::Candidate {
                        project_id: p.id,
                        owner: p.owner.clone(),
                        repo: p.repo.clone(),
                    })
                    .collect::<Vec<_>>(),
            ),
        }
    };
    match start_project::mode_for(ctx, &input) {
        Some(Mode::Assist) => {
            preview.suggested_project = start_project::ask(ctx, &input).await;
            preview.proposal = preview
                .suggested_project
                .as_ref()
                .map(start_project::SuggestedProject::proposal);
        }
        Some(Mode::Shadow) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                start_project::ask(&ctx, &input).await;
            });
        }
        None => {}
    }
    Ok(preview)
}

/// [`preview_start_decided`]'s N3 half: with a planned project, ask
/// `sibling_repos` over the projects the key ran in before (assist awaited
/// into `suggested_sibling`, shadow spawned and only recorded).
async fn suggest_sibling(
    store: &Mutex<Store>,
    ctx: &crate::service::decide::DecideCtx,
    preview: &mut StartPreview,
    plan: &StartPlan,
) -> Result<(), IpcError> {
    use crate::service::decide::{sibling_repos, start_project, Mode};
    let input = {
        let s = lock(store)?;
        let Some(chosen) = s.get_project(plan.project_id)? else {
            return Ok(());
        };
        let org_id = preview
            .item_id
            .map(|id| s.item_org(id))
            .transpose()?
            .flatten();
        let candidates = sibling_candidates(&s, &preview.key, org_id, plan.project_id)?;
        if candidates.is_empty() {
            return Ok(());
        }
        let description = preview
            .item_id
            .map(|id| s.work_item_meta(id))
            .transpose()?
            .and_then(|m| m.description);
        sibling_repos::SiblingInput {
            key: preview.key.clone(),
            title: preview.title.clone(),
            item_id: preview.item_id,
            org_id,
            description,
            chosen: start_project::Candidate {
                project_id: chosen.id,
                owner: chosen.owner,
                repo: chosen.repo,
            },
            candidates,
        }
    };
    match sibling_repos::mode_for(ctx, &input) {
        Some(Mode::Assist) => preview.suggested_sibling = sibling_repos::ask(ctx, &input).await,
        Some(Mode::Shadow) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                sibling_repos::ask(&ctx, &input).await;
            });
        }
        None => {}
    }
    Ok(())
}

/// Jev N3's follow-up after a PERSON's start of `key` in `primary` (and
/// `started`, every project of it): best effort, never an error.
fn record_sibling_start(
    store: &Mutex<Store>,
    item_id: Option<i64>,
    key: &str,
    primary: i64,
    started: &[i64],
) {
    if let Ok(s) = lock(store) {
        if let Err(e) = crate::service::decide::sibling_repos::record_start(
            &s,
            item_id,
            key,
            primary,
            started,
            crate::store::now_unix(),
        ) {
            tracing::warn!(
                "[decide] sibling_repos follow-up not recorded: {}",
                e.message
            );
        }
    }
}

/// Most projects a preview offers to pick from.
const PREVIEW_PROJECTS_MAX: usize = 8;

/// Preview a start: [`start_work`]'s path up to the spawn, with every
/// conflict collected instead of returned. A key that already has a live
/// session is planned as a parallel start (its own `-N` checkout), so the
/// popover can offer one; a cross-org start is planned as if forced, so the
/// person sees where it would land before they choose to. Refusals that are
/// not choices — a host fence, an org a bound client may not start in, a
/// checkout the caller may not land in — stay errors.
pub async fn preview_start(
    store: &Mutex<Store>,
    args: &StartArgs,
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<StartPreview, IpcError> {
    let args = with_native_defaults(store, args, &view.org)?;
    let mut conflicts = Vec::new();
    let ticket = resolve_ticket(store, &args, view, net, true).await?;
    let (projects, hosts, live, done, rule_offer) = {
        let s = lock(store)?;
        let mut projects = s.list_projects()?;
        projects.retain(|p| !p.system);
        projects.sort_by_key(|p| std::cmp::Reverse(p.last_session_at.unwrap_or(0)));
        let projects: Vec<ProjectChoice> = projects
            .into_iter()
            .take(PREVIEW_PROJECTS_MAX)
            .map(|p| ProjectChoice {
                id: p.id,
                owner: p.owner,
                repo: p.repo,
            })
            .collect();
        let local = crate::service::hub::local_host_enabled();
        let hosts: Vec<HostChoice> = s
            .list_hosts()?
            .into_iter()
            .filter(|h| crate::service::hosts::is_active(h, local))
            .filter(|h| view.org.host().is_none_or(|own| own == h.alias))
            .map(|h| HostChoice {
                alias: h.alias,
                reachable: h.reachable,
            })
            .collect();
        let item_org = ticket
            .item_id
            .map(|id| s.item_org(id))
            .transpose()?
            .flatten();
        let live: Vec<SessionRow> = live_work_on(&s, &ticket.key, item_org)?
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        let item = ticket
            .item_id
            .map(|id| s.get_work_item(id))
            .transpose()?
            .flatten();
        if let Some(item) = &item {
            if item.origin.as_deref() == Some("proposed")
                && item.proposal_state.as_deref() != Some("accepted")
            {
                conflicts.push(StartConflict {
                    kind: "proposal".into(),
                    message: "This is a proposal: accept it to start.".into(),
                    session_id: None,
                });
            }
        }
        let done = item.as_ref().is_some_and(|i| i.status_category == "done");
        let offer = crate::service::start_rules::offer_for(&s, view, item_org, &ticket.key)?;
        (projects, hosts, live, done, offer)
    };
    if let Some(row) = live.first() {
        let visible = view.sees_session_row(row).is_visible();
        conflicts.push(StartConflict {
            kind: "live_session".into(),
            message: if visible {
                format!(
                    "{} is already open in {} on {}.",
                    ticket.key,
                    row.friendly_name.as_deref().unwrap_or(&row.tmux_name),
                    row.host_alias
                )
            } else {
                "Someone is already working on this.".into()
            },
            session_id: visible.then_some(row.id),
        });
    }
    if done {
        conflicts.push(StartConflict {
            kind: "done".into(),
            message: "This task is done.".into(),
            session_id: None,
        });
    }
    let planned = StartArgs {
        parallel: args.parallel || !live.is_empty(),
        ..args.clone()
    };
    let mut preview = StartPreview {
        key: ticket.key.clone(),
        title: ticket.title.clone(),
        item_id: ticket.item_id,
        plan: None,
        missing: None,
        projects,
        hosts,
        conflicts,
        brief: None,
        checkout: None,
        suggested_project: None,
        proposal: None,
        suggested_sibling: None,
        rule_offer,
        brief_draft: None,
    };
    let plan = match plan_resolved(store, &planned, view, &ticket) {
        Ok(p) => p,
        Err(e) if e.code == codes::E_AMBIGUOUS => {
            preview.missing = e
                .details
                .as_ref()
                .and_then(|d| d.get("missing"))
                .and_then(|m| m.as_str())
                .map(str::to_string);
            return Ok(preview);
        }
        Err(e)
            if e.code == codes::E_FORBIDDEN
                && e.details
                    .as_ref()
                    .and_then(|d| d.get("cross_org"))
                    .and_then(|v| v.as_bool())
                    == Some(true) =>
        {
            preview.conflicts.push(StartConflict {
                kind: "cross_org".into(),
                message: e.message.clone(),
                session_id: None,
            });
            let forced = StartArgs {
                force_cross_org: true,
                ..planned.clone()
            };
            plan_resolved(store, &forced, view, &ticket)?
        }
        Err(e) if e.code == codes::E_EXISTS => {
            // A multi-repo sibling's branch already has a live session.
            preview.conflicts.push(StartConflict {
                kind: "worktree_busy".into(),
                message: e.message.clone(),
                session_id: e
                    .details
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_i64()),
            });
            return Ok(preview);
        }
        Err(e) => return Err(e),
    };
    preview.brief = match (&args.brief, args.with_brief) {
        (Some(b), _) => Some(b.clone()),
        (None, true) if brief_visible_on(store, &plan)? => Some(ticket_brief(store, &plan)?),
        (None, _) => None,
    };
    if let Some(wid) = plan.worktree_id {
        let s = lock(store)?;
        let busy = s
            .list_sessions_for_host(&plan.host_alias)?
            .into_iter()
            .find(|r| r.worktree_id == Some(wid) && r.status == "running" && r.lost_at.is_none());
        let busy_by = busy
            .as_ref()
            .filter(|r| view.sees_session_row(r).is_visible())
            .map(|r| r.id);
        if let Some(r) = &busy {
            preview.conflicts.push(StartConflict {
                kind: "worktree_busy".into(),
                message: format!(
                    "The checkout {} is in use by a live session; a parallel start takes its own.",
                    plan.branch
                ),
                session_id: busy_by.filter(|id| *id == r.id),
            });
        }
        preview.checkout = Some(CheckoutPreview {
            exists: true,
            busy_by,
        });
    } else {
        preview.checkout = Some(CheckoutPreview {
            exists: false,
            busy_by: None,
        });
    }
    preview.plan = Some(plan);
    Ok(preview)
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
    view: &crate::service::view_scope::ViewScope,
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
    let ticket = resolve_start(store, args, view, net).await?;
    // One claim for the whole batch (work graph M14), taken before the
    // siblings are planned so their guards see any start that won before
    // it: another start or resume of the key waits for the batch to end.
    let _claim = InFlight::claim(&*lock(store)?, &ticket.key)?;
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
        match plan_resolved(store, &one, view, &ticket) {
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
            tokio::time::timeout_at(deadline, start_one(store, plan, brief, view, &mut spawn))
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
    // Jev N3's follow-up: the repositories a person chose to start the key
    // in (the first is the dialog's own project, the rest its ticked
    // siblings) answer a sibling proposal they were shown. What they chose,
    // not what came up: a sibling that failed to start was still chosen.
    if args.decider == Decider::Person {
        record_sibling_start(store, ticket.item_id, &ticket.key, ids[0], &ids);
        // Redesign 8.11: the dialog's own project counts toward a rule.
        crate::service::start_rules::tally_logged(store, ticket.item_id, &ticket.key, ids[0]);
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
    view: &crate::service::view_scope::ViewScope,
    net: &TrackerNet,
) -> Result<MultiStart, IpcError> {
    start_many(
        store,
        args,
        project_ids,
        view,
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
