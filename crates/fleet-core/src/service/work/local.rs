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
//! **Who may WRITE one** is a narrower question, answered at the tool layer
//! by `support::require_drive_on_item_sessions`: every live session linked
//! to the item must be this caller's to drive, every ended link must be one
//! it may see — and an item with no CONFIRMED link at all is refused to
//! everybody but a per-host token and a one-person install, because there
//! is then no record of whose work it was (multi-user M1, T9e). An item
//! whose last link was unlinked therefore stops being renameable by its own
//! owner too; it is still listed, and linking a session to it again makes
//! it writable again.
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
///
/// The ORG half only. Two callers, and they take different halves on top of
/// it: [`person_visible_links`] is the half every READ of a count takes, and
/// [`local_item_visible`] — the "does this item exist for this caller"
/// question, which stays on this org half because what it answers is about
/// the ITEM — has its person half at the tool layer instead
/// (`support::require_drive_on_item_sessions`).
///
/// [`local_item_visible`] has THREE call sites, not two (T9e corrected an
/// undercount here and in `scope_guard_tests.rs`): [`rename_local_item`],
/// `status::set_status`, and [`name_session_work_as`]. The first two carry
/// the tool-layer person gate; the third does not, and what fences it is
/// written at the guard itself.
fn visible_links(
    s: &Store,
    scope: &OrgScope,
    links: Vec<LocalItemLink>,
) -> Result<Vec<LocalItemLink>, IpcError> {
    // This is the org boundary, not a privacy fence: the doc above says the same thing at
    // length: this is the ORG half, and `person_visible_links` is the half every READ of a count
    // takes on top of it.
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

/// [`visible_links`], then the PERSON fence
/// ([`orgs::link_person_visible`]) — multi-user M1, T8d.
///
/// `LocalWorkItem::live_sessions` is the "someone is working on this" bit, and
/// `Graph::build` person-fences that exact signal one directory away,
/// deliberately and with a comment saying why (`working_session_items`). Here
/// it was counted over every link that survived `visible_links`' first line,
/// `if scope.is_all() { return Ok(links) }` — and `All` is what every paired
/// client bound to no org resolves to, so a second person's phone was told how
/// many live sessions the fleet was running on each named item. Rule 6's
/// allowance is a per-host count of `unclaimed` rows, not a per-item count of
/// somebody's live work.
///
/// The ITEM is not fenced here: a local item's title and key are item data,
/// and the Work view already shows a task while dropping the links under it.
/// So this keeps the item and zeroes the count.
///
/// **One kind of local item is the exception, and the sentence above used to
/// be written as though there were none** (multi-user M1, T5's review): a JOB
/// MIRROR, whose title is the first line of the dispatch prompt
/// (`Store::create_agent_task_item`). Its title is withheld in
/// [`local_items`] itself, on the same fence every other surface of a mirror
/// asks (`view::visible_job_states`).
fn person_visible_links(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    links: Vec<LocalItemLink>,
) -> Result<Vec<LocalItemLink>, IpcError> {
    let kept = visible_links(s, &view.org, links)?;
    if view.is_internal() {
        return Ok(kept);
    }
    let mut out = Vec::with_capacity(kept.len());
    for l in kept {
        if orgs::link_person_visible(s, view, &l.link)? {
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
pub fn local_items(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<LocalWorkItem>, IpcError> {
    let scope = &view.org;
    let s = lock(store)?;
    let mut by_item: BTreeMap<i64, Vec<LocalItemLink>> = BTreeMap::new();
    for l in person_visible_links(&s, view, s.local_item_links(None)?)? {
        by_item.entry(l.item_id).or_default().push(l);
    }
    // The dispatch fence, for the job mirrors among these items: this list
    // has no `Graph`, so it asks the same predicate directly.
    let jobs = crate::service::work::view::visible_job_states(&s, view)?;
    Ok(s.local_work_items()?
        .into_iter()
        .filter_map(|i| {
            let links = by_item.get(&i.id);
            // This is the org boundary, not a privacy fence: it decides whether the ITEM is
            // listed, and a local item's key and title are item data. The count beside it is
            // person-fenced, by `person_visible_links`.
            if !scope.is_all() && links.is_none() {
                return None;
            }
            // A job mirror's title is the dispatch PROMPT's first line, so it
            // is content of both ends of the dispatch and not item data. The
            // row stays — the key still names it, and that is what a caller
            // links by — under the same withheld label every other surface
            // uses.
            let hidden = i.origin.as_deref() == Some("agent") && !jobs.contains_key(&i.id);
            Some(LocalWorkItem {
                id: i.id,
                key: i.key,
                title: if hidden {
                    crate::service::work::view::JOB_TITLE_WITHHELD.to_string()
                } else {
                    i.title
                },
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
            // The org half. The person half is `resolve_row_person_gated(..,
            // Reach::Drive, ..)` in `work_link { action: name }`, taken
            // without the host gate in front so an unreachable session
            // answers as an unknown id.
            None => scope.sees_row_org_only(&r) && scope.sees_org(r.org_id),
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

/// `work_link { action: edit, item_id, title?, notes?, assignees?, due_at?,
/// epic? }`: a
/// person edits a local item (task editing). The same fences as
/// [`rename_local_item`]: a tracker's ticket is `E_INVALID` (its tracker
/// owns its text), and an item outside the scope answers as an unknown id.
pub fn edit_local_item(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<WorkItemRow, IpcError> {
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "edit needs item_id"))?;
    if args.title.is_none()
        && args.notes.is_none()
        && args.assignees.is_none()
        && args.due_at.is_none()
        && args.epic.is_none()
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            "edit needs title, notes, assignees, due_at or epic",
        ));
    }
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
            format!(
                "{} is a tracker's ticket; edit it in its tracker",
                item.key.as_deref().unwrap_or("this item")
            ),
        ));
    }
    if !local_item_visible(&s, scope, id)? {
        return Err(orgs::not_found("work item", id));
    }
    // The epic flag first: its refusal (an item under a parent) leaves the
    // rest of the edit unwritten too.
    if let Some(epic) = args.epic {
        s.set_local_epic(id, epic)?
            .ok_or_else(|| orgs::not_found("work item", id))?;
    }
    s.edit_local_item(
        id,
        &crate::store::ItemEdit {
            title: args.title.as_deref(),
            notes: args.notes.as_deref(),
            assignees: args.assignees.as_deref(),
            due_at: args.due_at.as_deref(),
        },
    )?
    .ok_or_else(|| orgs::not_found("work item", id))
}

/// `work_link { action: set_parent, item_id, parent }` (sprints design
/// 2026-09-28 §3): a person files a local item under an epic or a task
/// (`parent: "item:<id>"`), or takes it out to the top (`parent: ""`). The
/// fences of [`edit_local_item`]: a tracker's ticket is `E_INVALID` (its
/// tracker files it), and an item or a parent outside the scope answers as
/// an unknown id. The rules of the hierarchy are
/// [`Store::set_local_parent`]'s.
pub fn set_parent(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<WorkItemRow, IpcError> {
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "set_parent needs item_id"))?;
    let parent = match args.parent.as_deref().map(str::trim) {
        None => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "set_parent needs parent: item:<id>, or \"\" for the top",
            ))
        }
        Some("") => None,
        Some(_) => parent_id(args)?,
    };
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
            format!(
                "{} is a tracker's ticket; its tracker files it",
                item.key.as_deref().unwrap_or("this item")
            ),
        ));
    }
    if !local_item_visible(&s, scope, id)? {
        return Err(orgs::not_found("work item", id));
    }
    if let Some(p) = parent {
        // This is the org boundary, not a privacy fence: it asks whether the
        // PARENT ITEM exists for this caller, as `create_task`'s does, and an
        // item's key and title are work data.
        if !scope.is_all() {
            visible_parent(&s, scope, p)?;
        }
    }
    s.set_local_parent(id, parent)?
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

/// `work_link { action: create, title, parent?, project_id?, notes?,
/// assignees?, due_at? }` (shared work context, design 2026-09-29; the owner
/// and due date, Orbit Fleet M15). A standalone task needs an
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
    // Checked before anything is written: a bad date or name refuses the
    // whole create rather than leave a task without them.
    if let Some(a) = args.assignees.as_deref() {
        crate::store::validate_assignees(a)?;
    }
    if let Some(d) = args.due_at.as_deref() {
        crate::store::validate_due_date(d)?;
    }
    let s = lock(store)?;
    match parent {
        // This is the org boundary, not a privacy fence: a standalone task has
        // no links and no sessions, so there is no row and nobody to fence —
        // the question is authority to add top-level work.
        None if !scope.is_all() => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a standalone task needs an unscoped caller (the desktop or the master \
                 token); add a subtask under a task you work on instead",
            ))
        }
        // This is the org boundary, not a privacy fence: it asks whether the
        // PARENT ITEM exists for this caller, and an item's key and title are
        // work data that survive the person fence (as `task_visible`'s do).
        Some(p) if !scope.is_all() => visible_parent(&s, scope, p)?,
        _ => {}
    }
    let item = s.create_native_item(&crate::store::NativeItem {
        title,
        parent_id: parent,
        project_id: args.project_id,
        notes: args.notes.as_deref(),
    })?;
    if args.assignees.is_none() && args.due_at.is_none() {
        return Ok(item);
    }
    s.edit_local_item(
        item.id,
        &crate::store::ItemEdit {
            assignees: args.assignees.as_deref(),
            due_at: args.due_at.as_deref(),
            ..Default::default()
        },
    )?
    .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after create"))
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
    // This is the org boundary, not a privacy fence: the same parent-ITEM
    // question `create_task` asks. The person half of a proposal is at the
    // tool layer, where the session that gets the credit passes
    // `resolve_target_row` at `Reach::Drive` before `proposer` is built.
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

/// `work_link { action: propose_tree, parent, tree }` (orchestration O2):
/// several subtasks with the edges between them, for a person to accept in
/// one go. `proposer` is the caller's session label, as for `propose`.
pub fn propose_tree(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    proposer: &str,
) -> Result<Vec<WorkItemRow>, IpcError> {
    let tree = args
        .tree
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "propose_tree needs tree"))?;
    let parent = parent_id(args)?
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "propose_tree needs parent"))?;
    let s = lock(store)?;
    // This is the org boundary, not a privacy fence: `propose`'s parent-ITEM
    // question, asked of the parent and of every existing item an entry
    // waits for; the person half is at the tool layer, as for `propose`.
    if !scope.is_all() {
        visible_parent(&s, scope, parent)?;
        for e in tree {
            for d in &e.depends_on {
                if let crate::store::TreeRef::Item(id) = d {
                    visible_parent(&s, scope, *id)?;
                }
            }
        }
    }
    s.propose_tree(parent, tree, proposer)
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
    // This is the org boundary, not a privacy fence: deciding a proposal is a
    // PERSON's act by design (an agent never accepts its own), so every scoped
    // caller is refused outright and no row is reached to fence.
    if !scope.is_all() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a person decides proposals, from the desktop or the master token",
        ));
    }
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "accept / reject needs item_id"))?;
    // Redesign 6.9: a reject naming `task_id: item:<id>` is Merge — what
    // hangs on the proposal moves to that task before the proposal closes.
    // A person's act like every decision here (the fence above), and only
    // ever on their click: nothing merges by itself.
    let merge_into = match (accept, args.task_id.as_deref()) {
        (false, Some(raw)) => Some(
            raw.strip_prefix("item:")
                .and_then(|n| n.parse::<i64>().ok())
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "merge into item:<id>"))?,
        ),
        _ => None,
    };
    let s = lock(store)?;
    let row = match merge_into {
        Some(into) => {
            s.merge_proposal_into(id, into)?;
            s.get_work_item(id)?
                .ok_or_else(|| orgs::not_found("work item", id))?
        }
        None => s.decide_proposal(id, accept)?,
    };
    // K4's follow-up: Merge (reject) confirms Jev's "may duplicate", Keep
    // both (accept) rejects it. Never fails the decision.
    if let Err(e) = crate::service::decide::duplicate::record_decision(
        &s,
        id,
        accept,
        crate::service::catalog::now_secs(),
    ) {
        tracing::warn!("[decide] duplicate follow-up not recorded: {}", e.message);
    }
    Ok(row)
}

/// A local item is visible: always to `All`; to a per-host token through
/// one of its visible links.
///
/// `pub(super)`: `service::work::status::set_status` reuses this exact fence
/// rather than a second copy of it (a tracker item's is `Store::item_org` +
/// `OrgScope::sees_org`, applied at that call site the way `rename_local_item`
/// below does).
///
/// THREE call sites, enumerated at the guard inside: the two writes
/// ([`rename_local_item`], `status::set_status`) and
/// [`name_session_work_as`], which has no item-level person gate. Read that
/// comment before adding a fourth.
pub(super) fn local_item_visible(
    s: &Store,
    scope: &OrgScope,
    item_id: i64,
) -> Result<bool, IpcError> {
    // This is the org boundary, not a privacy fence: what this answers is
    // about the ITEM, as the doc above says.
    //
    // THREE call sites, enumerated because the count itself has been wrong
    // twice. The two WRITES — `local::rename_local_item` and
    // `status::set_status` — carry the person fence
    // `require_drive_on_item_sessions`, threaded at the tool layer
    // (`mcp/tools/orchestration.rs`'s `name` and `set_status` arms). T9c's
    // row said "the two writes" while the gate was on the rename half only,
    // so `set_status` had no person gate at all and a stranger could mark
    // another person's work done (fixed in T9d); T9d's row then still said
    // "both writes that ask it" while a THIRD site existed (T9e).
    //
    // The third site is `local::name_session_work_as`, and it has NO
    // item-level person gate — stated here rather than left to be
    // rediscovered. What fences it: the caller has already been gated at
    // `Reach::Drive` on its OWN session (orchestration.rs's
    // `resolve_row_person_gated` in the `name` + `session_id` arm), and the
    // only item-derived output under this shortcut is the `item_id` in an
    // `E_EXISTS` reply — an id `work { local_items }` deliberately lists to
    // every caller, because a local item's key and title are item data and
    // only the live-session COUNT beside them is person-fenced
    // (`person_visible_links`). So the shortcut discloses nothing there that
    // the same caller cannot list by name. A write at that site would need
    // the gate; a disclosure wider than an id would too.
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

/// `work_link { action: comment, item_id, notes }`: a comment on a task the
/// caller sees — fleet's own or a tracker's ticket (the comment stays in
/// fleet). An item outside the scope answers as an unknown id.
pub fn comment(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    author: &str,
    person: Option<i64>,
) -> Result<crate::store::CommentRow, IpcError> {
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "comment needs item_id"))?;
    let body = args
        .notes
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "comment needs notes: its text"))?;
    let s = lock(store)?;
    visible_parent(&s, scope, id)?;
    let mut c = s.add_comment(id, author, person, body)?;
    c.mine = true;
    Ok(c)
}

/// `work_link { action: comment_delete, comment_id }`: its author's alone.
/// A comment on an item outside the scope answers as an unknown one.
pub fn comment_delete(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    author: &str,
    person: Option<i64>,
) -> Result<crate::store::CommentRow, IpcError> {
    let id = args
        .comment_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "comment_delete needs comment_id"))?;
    let unknown = || IpcError::new(codes::E_NOTFOUND, format!("comment {id} not found"));
    let s = lock(store)?;
    let c = s.get_comment(id)?.ok_or_else(unknown)?;
    visible_parent(&s, scope, c.item_id).map_err(|_| unknown())?;
    s.delete_comment(id, person, author)?.ok_or_else(unknown)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod due_tests {
    use super::*;

    fn args(action: &str) -> WorkLinkArgs {
        WorkLinkArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_task_is_created_with_its_owner_and_due_date_and_edited_to_another() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let t = create_task(
            &WorkLinkArgs {
                title: Some("Rotate NAS sudo password".into()),
                assignees: Some(vec!["Ana".into()]),
                due_at: Some("2026-10-16".into()),
                ..args("create")
            },
            &store,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(
            (t.assignees.clone(), t.due_at.as_deref()),
            (vec!["Ana".to_string()], Some("2026-10-16"))
        );
        let e = edit_local_item(
            &WorkLinkArgs {
                item_id: Some(t.id),
                due_at: Some("2026-10-23".into()),
                ..args("edit")
            },
            &store,
            &OrgScope::All,
        )
        .unwrap();
        assert_eq!(
            (e.assignees, e.due_at.as_deref()),
            (vec!["Ana".to_string()], Some("2026-10-23"))
        );
    }

    #[test]
    fn a_bad_due_date_refuses_the_create_and_writes_no_task() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let before = lock(&store).unwrap().local_work_items().unwrap().len();
        let e = create_task(
            &WorkLinkArgs {
                title: Some("Ship".into()),
                due_at: Some("Friday".into()),
                ..args("create")
            },
            &store,
            &OrgScope::All,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert_eq!(
            lock(&store).unwrap().local_work_items().unwrap().len(),
            before
        );
    }

    #[test]
    fn a_trackers_ticket_keeps_its_trackers_due_date() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = lock(&store).unwrap();
            let t = s
                .add_tracker("jira", "Acme", "https://acme.atlassian.net")
                .unwrap();
            s.upsert_tracker_item(
                t.id,
                &crate::store::TrackerItemWrite {
                    external_id: "1".into(),
                    key: Some("ABC-1".into()),
                    title: "Ticket".into(),
                    status_name: "To Do".into(),
                    status_category: "todo".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        };
        let e = edit_local_item(
            &WorkLinkArgs {
                item_id: Some(id),
                due_at: Some("2026-10-16".into()),
                ..args("edit")
            },
            &store,
            &OrgScope::All,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert_eq!(
            lock(&store)
                .unwrap()
                .get_work_item(id)
                .unwrap()
                .unwrap()
                .due_at,
            None
        );
    }
}
