//! "Name this work…" (work graph M11.1): local work items — work with a
//! title and no tracker ticket — named from a session, renamed, and listed.
//! `work_link { action: name }` and `work { action: local_items }` call
//! these, and so do the desktop's Routed commands.
//!
//! **Who sees a local item.** A local item has no org of its own; each link
//! to it takes its session's. The master, a paired client and the desktop
//! see every item. A per-host token sees an item only through the work its
//! own host's sessions do, inside its org — a live link on one of its host's
//! sessions, or an ended one whose session ran there (the same fence as
//! `orgs::require_key`). An item outside that answers exactly as an item id
//! that does not exist.
//!
//! **A key that is taken.** Naming work with a key a tracker item the
//! caller can see already carries (by key or alias) is refused with
//! `E_EXISTS` — that work has a ticket; link it (`work_link { action: link,
//! key }`). A tracker item the caller cannot see is not a collision: the
//! answer is the one an unknown key gets, so it says nothing about the other
//! org. A key a local item already carries is refused with `E_EXISTS` for
//! every caller (local keys are one fleet-wide namespace, which
//! `work_link { action: link, key }` already resolves for anyone).

use super::{detect, WorkLinkArgs};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{Decider, LocalItemLink, SessionRow, Store, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// One row of `work { action: local_items }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalWorkItem {
    pub id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    /// Live sessions linked to it (for a per-host token: on its host).
    #[serde(default)]
    pub live_sessions: u32,
}

/// The links of one item this scope may count: all of them for `All`; for
/// a per-host token, live links on its own host's sessions and ended links
/// whose session ran there, inside its orgs.
fn visible_links(
    s: &Store,
    scope: &OrgScope,
    links: Vec<LocalItemLink>,
) -> Result<Vec<LocalItemLink>, IpcError> {
    if scope.is_all() {
        return Ok(links);
    }
    let Some(h) = scope.host() else {
        // A bound client (work graph M14): links inside its orgs whose
        // session it may see; no host fence.
        let mut out = Vec::with_capacity(links.len());
        for mut l in links {
            l.link.org_id = s.link_org(&l.link)?;
            if scope.sees_link(&l.link) && orgs::link_session_visible(s, scope, &l.link)? {
                out.push(l);
            }
        }
        return Ok(out);
    };
    let mut out = Vec::with_capacity(links.len());
    for mut l in links {
        l.link.org_id = s.link_org(&l.link)?;
        let on_host = match l.link.ended_at {
            None => l.session_host.as_deref() == Some(h),
            Some(_) => l.link.snap_host.as_deref() == Some(h),
        };
        if on_host && scope.sees_link(&l.link) {
            out.push(l);
        }
    }
    Ok(out)
}

fn live_count(links: &[LocalItemLink]) -> u32 {
    links
        .iter()
        .filter(|l| l.link.ended_at.is_none())
        .filter_map(|l| l.session_id)
        .collect::<BTreeSet<_>>()
        .len() as u32
}

/// `work { action: local_items }`: the local items this scope sees, most
/// recently changed first.
pub fn local_items(store: &Mutex<Store>, scope: &OrgScope) -> Result<Vec<LocalWorkItem>, IpcError> {
    let s = lock(store)?;
    let mut by_item: BTreeMap<i64, Vec<LocalItemLink>> = BTreeMap::new();
    for l in visible_links(&s, scope, s.local_item_links(None)?)? {
        by_item.entry(l.item_id).or_default().push(l);
    }
    Ok(s.local_work_items()?
        .into_iter()
        .filter_map(|i| {
            let links = by_item.get(&i.id);
            if !scope.is_all() && links.is_none() {
                return None;
            }
            Some(LocalWorkItem {
                id: i.id,
                key: i.key,
                title: i.title,
                created_at: i.created_at,
                updated_at: i.updated_at,
                live_sessions: links.map_or(0, |l| live_count(l)),
            })
        })
        .collect())
}

/// `work_link { action: name, session_id, title, key? }`: a new local item,
/// linked to the session (confirmed, `manual` — `agent` for an agent;
/// primary when the session has no primary work). Returns the session's
/// updated row.
///
/// A per-host token names work only on its own host's sessions inside its
/// org; any other session id answers exactly as one that does not exist.
///
/// A PERSON's naming (the desktop's); an agent's goes through
/// [`name_session_work_as`].
pub fn name_session_work(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<SessionRow, IpcError> {
    name_session_work_as(args, store, scope, Decider::Person)
}

/// [`name_session_work`] by `decider`, which decides the link's source
/// (D34): `manual` for a person, `agent` for an agent (a per-host token,
/// the operator) — an agent naming its work is never a person's link.
pub fn name_session_work_as(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    decider: Decider,
) -> Result<SessionRow, IpcError> {
    let sid = args
        .session_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "name needs session_id"))?;
    let title = required_title(args)?;
    let s = lock(store)?;
    let visible = s
        .get_session_by_id(sid)?
        .is_some_and(|r| match scope.host() {
            None => scope.sees_row(&r) && scope.sees_org(r.org_id),
            Some(h) => r.host_alias == h && scope.sees_org(r.org_id),
        });
    if !visible {
        return Err(orgs::not_found("session", sid));
    }
    let key = args
        .key
        .as_deref()
        .map(crate::store::normalize_work_ref)
        .transpose()?;
    if let Some(k) = key.as_deref() {
        for item in s.tracker_items_with_key(k)? {
            if scope.sees_org(s.item_org(item.id)?) {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!(
                        "{k} is a tracker's ticket, not new work: link the session to it \
                         (work_link {{ action: link, key }}) instead of naming it"
                    ),
                )
                .with_details(serde_json::json!({ "item_id": item.id, "tracker": true })));
            }
        }
        if let Some(existing) = s.local_work_item_by_key(k)? {
            let mut err = IpcError::new(
                codes::E_EXISTS,
                format!(
                    "work key {k} already names local work: link the session to it \
                     (work_link {{ action: link, key }}) or pick another key"
                ),
            );
            if local_item_visible(&s, scope, existing.id)? {
                err = err.with_details(serde_json::json!({ "item_id": existing.id }));
            }
            return Err(err);
        }
    }
    s.name_session_work_by(sid, key.as_deref(), &title, decider)?;
    // A new primary can settle a pending suggestion (M4.3), as any decision.
    if let Err(e) = detect::resolve_session(&s, sid) {
        tracing::debug!(error = %e.message, "[work] resolve after naming work failed");
    }
    s.get_session_by_id(sid)?
        .ok_or_else(|| orgs::not_found("session", sid))
}

/// `work_link { action: name, item_id, title }`: rename a LOCAL item.
/// A tracker's ticket the caller can see is refused with `E_INVALID`; an
/// item outside the scope answers as an unknown id.
pub fn rename_local_item(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<WorkItemRow, IpcError> {
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "rename needs item_id"))?;
    let title = required_title(args)?;
    let s = lock(store)?;
    let Some(item) = s.get_work_item(id)? else {
        return Err(orgs::not_found("work item", id));
    };
    if item.source != "local" {
        if !scope.sees_org(s.item_org(id)?) {
            return Err(orgs::not_found("work item", id));
        }
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("work item {id} is a tracker's ticket; only local work can be renamed"),
        ));
    }
    if !local_item_visible(&s, scope, id)? {
        return Err(orgs::not_found("work item", id));
    }
    s.rename_local_work_item(id, &title)?
        .ok_or_else(|| orgs::not_found("work item", id))
}

/// `item:<id>` → the id.
fn parent_id(args: &WorkLinkArgs) -> Result<Option<i64>, IpcError> {
    match args.parent.as_deref() {
        None => Ok(None),
        Some(raw) => raw
            .strip_prefix("item:")
            .and_then(|n| n.parse::<i64>().ok())
            .map(Some)
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "parent is item:<id>")),
    }
}

/// The parent a scoped caller may add under: an item it can see. One it
/// cannot answers exactly as an id that does not exist.
fn visible_parent(s: &Store, scope: &OrgScope, id: i64) -> Result<(), IpcError> {
    let Some(item) = s.get_work_item(id)? else {
        return Err(orgs::not_found("work item", id));
    };
    let visible = if item.source == "local" {
        local_item_visible(s, scope, id)?
    } else {
        crate::service::trackers::tickets::item_visible(scope, s, &item)?
    };
    if visible {
        Ok(())
    } else {
        Err(orgs::not_found("work item", id))
    }
}

/// `work_link { action: create, title, parent?, project_id?, notes? }`
/// (shared work context, design 2026-09-29). A standalone task needs an
/// unscoped caller (a new item has no links, and a scoped caller sees a
/// local item only through its links); a subtask needs a parent the caller
/// sees.
pub fn create_task(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<WorkItemRow, IpcError> {
    let title = args
        .title
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "create needs title"))?;
    let parent = parent_id(args)?;
    let s = lock(store)?;
    match parent {
        None if !scope.is_all() => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a standalone task needs an unscoped caller (the desktop or the master \
                 token); add a subtask under a task you work on instead",
            ))
        }
        Some(p) if !scope.is_all() => visible_parent(&s, scope, p)?,
        _ => {}
    }
    s.create_native_item(&crate::store::NativeItem {
        title,
        parent_id: parent,
        project_id: args.project_id,
        notes: args.notes.as_deref(),
    })
}

/// `work_link { action: propose, parent, title, notes?, why? }`: a subtask
/// for a person to accept or reject. `proposer` is the caller's session
/// label, never an argument.
pub fn propose(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    proposer: &str,
) -> Result<WorkItemRow, IpcError> {
    let title = args
        .title
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "propose needs title"))?;
    let parent =
        parent_id(args)?.ok_or_else(|| IpcError::new(codes::E_INVALID, "propose needs parent"))?;
    let s = lock(store)?;
    if !scope.is_all() {
        visible_parent(&s, scope, parent)?;
    }
    s.propose_subtask(&crate::store::Proposal {
        parent_id: parent,
        title,
        notes: args.notes.as_deref(),
        why: args.why.as_deref(),
        proposed_by: proposer,
    })
}

/// `work_link { action: accept | reject, item_id }` (no `session_id`): a
/// person's decision on a proposal. Refused to per-host tokens and bound
/// clients: an agent never accepts its own (or any) proposal.
pub fn decide(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    accept: bool,
) -> Result<WorkItemRow, IpcError> {
    if !scope.is_all() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a person decides proposals, from the desktop or the master token",
        ));
    }
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "accept / reject needs item_id"))?;
    lock(store)?.decide_proposal(id, accept)
}

/// A local item is visible: always to `All`; to a per-host token through
/// one of its visible links.
///
/// `pub(super)`: `service::work::status::set_status` reuses this exact fence
/// rather than a second copy of it (a tracker item's is `Store::item_org` +
/// `OrgScope::sees_org`, applied at that call site the way `rename_local_item`
/// below does).
pub(super) fn local_item_visible(
    s: &Store,
    scope: &OrgScope,
    item_id: i64,
) -> Result<bool, IpcError> {
    if scope.is_all() {
        return Ok(true);
    }
    Ok(!visible_links(s, scope, s.local_item_links(Some(item_id))?)?.is_empty())
}

fn required_title(args: &WorkLinkArgs) -> Result<String, IpcError> {
    crate::store::validate_local_work_title(
        args.title
            .as_deref()
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "name needs title"))?,
    )
}

#[cfg(test)]
mod tests;
