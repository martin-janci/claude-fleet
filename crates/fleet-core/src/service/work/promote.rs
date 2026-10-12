//! Promoting a task made in fleet to a tracker's ticket (owner decision
//! 2026-10-11): `work_link { action: promote, item_id, tracker_id?,
//! container? }`.
//!
//! A person keeps work local while it is small — a task, or a subtask under
//! a Jira or Asana ticket — and promotes it once it should be tracked. Fleet
//! then creates the ticket and the task BECOMES it: the same row, so its
//! sessions, comments, subtasks, sprints and history stay put, and its old
//! fleet key still resolves as an alias (`Store::promote_local_item`).
//!
//! **Where it goes.** A subtask of a tracker's ticket is created as that
//! ticket's subtask, in the ticket's own tracker (a Jira sub-task, an Asana
//! subtask, a Linear sub-issue, a GitHub sub-issue). A top-level task goes
//! to the tracker the person names (`tracker_id`), into `container`: a Jira
//! project key or Linear team key (one the site owns; optional when it owns
//! exactly one), an Asana project gid (one the tracker reads; none puts it
//! in the workspace, assigned to the API user), or a GitHub `owner/repo`
//! (one of the repositories the tracker is narrowed to, else any in its
//! scope). A task under a task of fleet's own is refused: promote the parent
//! first, or take the task out to the top. Epics, delegated jobs and open
//! proposals are refused too.
//!
//! **Who may.** A person only (a per-host token is an agent: `E_FORBIDDEN`),
//! and only where the tracker's admin turned `write_back.create_tickets` on
//! (off by default: `E_INVALID_STATE`), never across orgs. The create goes
//! through the write-back outbox (`trackers::write_back`), sent once by the
//! sync of whichever desktop or hub owns the trackers; the task page shows
//! it pending, created, or why it was not ([`PromotionView`]).

use super::local::{fleet_signature, visible_parent};
use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::trackers;
use crate::store::{is_epic, NewTrackerWrite, Store, TrackerRow, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// A task's promotion as its page shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionView {
    /// `pending` | `failed` (`done` is not shown: the task is the ticket).
    pub state: String,
    /// The tracker, by the name Settings gives it.
    pub tracker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One tracker a task may be promoted to, and where in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteTarget {
    pub tracker_id: i64,
    pub tracker: String,
    pub provider: String,
    /// The ticket the new one goes under (a subtask's parent); then no
    /// container is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    /// Where a top-level task may go, as `(id, name)`: Jira project keys,
    /// Asana projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<(String, String)>,
    /// A container must be picked (Jira); Asana may leave it to the
    /// workspace.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub container_required: bool,
}

/// PURE: the reason `item` is not one fleet promotes, or `None`.
fn not_promotable(item: &WorkItemRow) -> Option<&'static str> {
    if item.source != "local" {
        Some("it is already a tracker's ticket")
    } else if item.origin.as_deref() == Some("agent") {
        Some("it is a delegated job; it stays under the task it runs for")
    } else if matches!(
        item.proposal_state.as_deref(),
        Some("proposed" | "rejected")
    ) {
        Some("it is a proposal; accept it first")
    } else if is_epic(item) {
        Some("it is an epic; an epic stays fleet's own")
    } else if item.title.trim().is_empty() {
        Some("it has no title")
    } else {
        None
    }
}

/// The containers a top-level task may go into on `t`, and whether one
/// must be picked.
fn containers(t: &TrackerRow) -> (Vec<(String, String)>, bool) {
    match t.provider.as_str() {
        "asana" => (t.config.projects.clone(), false),
        // The repositories the tracker is narrowed to; with none, any
        // repository in its scope is typed in.
        "github" => (
            t.settings
                .repos
                .iter()
                .map(|r| (r.clone(), r.clone()))
                .collect(),
            true,
        ),
        // Jira project keys, Linear team keys.
        _ => (
            t.config
                .key_prefixes
                .iter()
                .map(|k| (k.clone(), k.clone()))
                .collect(),
            true,
        ),
    }
}

/// Where `item` may be promoted for a caller in `scope`: its parent's
/// tracker for a subtask of a ticket, every tracker that takes new tickets
/// for a top-level task, nothing otherwise (or for an agent).
pub fn targets(
    s: &Store,
    scope: &OrgScope,
    item: &WorkItemRow,
    trackers_by_id: &std::collections::HashMap<i64, TrackerRow>,
) -> Result<Vec<PromoteTarget>, IpcError> {
    if !super::tracker_comments::may_post(scope) || not_promotable(item).is_some() {
        return Ok(Vec::new());
    }
    let item_org = s.item_org(item.id)?;
    let fits = |t: &TrackerRow| {
        // This is the org boundary, not a privacy fence: which org's
        // tracker may hold this work is an org-authority question.
        trackers::write_back::writes_creates(t)
            && scope.sees_org(t.org_id)
            && !crosses(item_org, t.org_id)
    };
    if let Some(pid) = item.parent_id {
        let Some(parent) = s.get_work_item(pid)? else {
            return Ok(Vec::new());
        };
        let t = match (parent.source.as_str(), parent.tracker_id) {
            ("local", _) | (_, None) => None,
            (_, Some(t)) => trackers_by_id.get(&t),
        };
        return Ok(t
            .filter(|t| fits(t))
            .map(|t| PromoteTarget {
                tracker_id: t.id,
                tracker: t.name.clone(),
                provider: t.provider.clone(),
                parent_key: parent.key.clone(),
                ..Default::default()
            })
            .into_iter()
            .collect());
    }
    let mut out: Vec<PromoteTarget> = trackers_by_id
        .values()
        .filter(|t| fits(t))
        .map(|t| {
            let (containers, container_required) = containers(t);
            PromoteTarget {
                tracker_id: t.id,
                tracker: t.name.clone(),
                provider: t.provider.clone(),
                parent_key: None,
                containers,
                container_required,
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.tracker, a.tracker_id).cmp(&(&b.tracker, b.tracker_id)));
    Ok(out)
}

/// The promotion of `item_id` the task page shows: a pending or failed
/// one, by its tracker's name. A sent one shows as the ticket itself.
pub fn promotion_view(
    s: &Store,
    item_id: i64,
    trackers_by_id: &std::collections::HashMap<i64, TrackerRow>,
) -> Result<Option<PromotionView>, IpcError> {
    Ok(s.promotion_of(item_id)?
        .filter(|w| w.state != "done")
        .map(|w| PromotionView {
            tracker: trackers_by_id
                .get(&w.tracker_id)
                .map(|t| t.name.clone())
                .unwrap_or_default(),
            error: (w.state == "failed").then_some(w.last_error).flatten(),
            state: w.state,
        }))
}

/// Both orgs known and different.
fn crosses(a: Option<i64>, b: Option<i64>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a != b)
}

/// `work_link { action: promote, item_id, tracker_id?, container? }`: queue
/// the ticket that task `item_id` becomes. Returns the task as it stands
/// (still fleet's own until the tracker answers). The module docs say where
/// it goes and who may; an item outside the scope answers as an unknown id.
pub fn promote(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
    person: Option<i64>,
) -> Result<WorkItemRow, IpcError> {
    let id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "promote needs item_id"))?;
    let s = lock(store)?;
    visible_parent(&s, scope, id)?;
    if !super::tracker_comments::may_post(scope) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "only a person promotes a task to a tracker",
        ));
    }
    let item = s
        .get_work_item(id)?
        .ok_or_else(|| orgs::not_found("work item", id))?;
    let label = item.key.clone().unwrap_or_else(|| format!("item {id}"));
    if let Some(why) = not_promotable(&item) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{label} cannot be promoted: {why}"),
        ));
    }
    if s.promotion_of(id)?.is_some_and(|w| w.state == "pending") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{label} is already being created at its tracker"),
        ));
    }
    let tracker_of = |tid: i64| -> Result<TrackerRow, IpcError> {
        s.get_tracker(tid)?
            // This is the org boundary, not a privacy fence: a tracker of
            // another org answers as one that does not exist.
            .filter(|t| scope.sees_org(t.org_id))
            .ok_or_else(|| orgs::not_found("tracker", tid))
    };
    // Where: a subtask of a ticket goes under it, in its tracker; a
    // top-level task into the container picked on the tracker named.
    let (t, item_key, parent_external_id) = match item.parent_id {
        Some(pid) => {
            let parent = s
                .get_work_item(pid)?
                .ok_or_else(|| orgs::not_found("work item", pid))?;
            let parent_label = parent.key.clone().unwrap_or_else(|| format!("item {pid}"));
            let (Some(tid), Some(key)) = (parent.tracker_id, parent.key.clone()) else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{label} is under {parent_label}, which is fleet's own: promote \
                         {parent_label} first, or take {label} out to the top"
                    ),
                ));
            };
            if parent.source == "local" {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{parent_label} is fleet's own: promote it first"),
                ));
            }
            if args.tracker_id.is_some_and(|t| t != tid) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "{label} is a subtask of {parent_label}; it goes to that ticket's tracker"
                    ),
                ));
            }
            let t = tracker_of(tid)?;
            let ext = parent.external_id.clone();
            let ext = match t.provider.as_str() {
                // Jira files a sub-task by its parent's key; the outbox row
                // still marks it a subtask by carrying the parent's id.
                "jira" | "jira_dc" => ext.or_else(|| Some(key.clone())),
                _ => ext,
            }
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_UNSUPPORTED,
                    format!("{parent_label} has no tracker id to file a subtask under"),
                )
            })?;
            (t, key, Some(ext))
        }
        None => {
            let tid = args.tracker_id.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    "promote needs tracker_id for a task at the top",
                )
            })?;
            let t = tracker_of(tid)?;
            let (choices, required) = containers(&t);
            let picked = args
                .container
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty());
            let container = match picked {
                Some(c) => {
                    let (c, shape) = match t.provider.as_str() {
                        "asana" => (
                            c.to_string(),
                            c.len() <= 24 && c.bytes().all(|b| b.is_ascii_digit()),
                        ),
                        "github" => match trackers::github::normalize_repo(c) {
                            Some(r) => (r, true),
                            None => (c.to_string(), false),
                        },
                        "linear" => {
                            let c = c.to_ascii_uppercase();
                            let ok = trackers::linear::team_key_ok(&c);
                            (c, ok)
                        }
                        _ => {
                            let c = c.to_ascii_uppercase();
                            let ok = trackers::jira_common::is_project_key(&c);
                            (c, ok)
                        }
                    };
                    let known = choices.iter().any(|(id, _)| id.eq_ignore_ascii_case(&c));
                    if !shape || (!choices.is_empty() && !known) {
                        return Err(IpcError::new(
                            codes::E_INVALID,
                            format!("{} has no project {c:?}", t.name),
                        ));
                    }
                    c
                }
                None if required && choices.len() == 1 => choices[0].0.clone(),
                None if required => {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        format!(
                            "promote needs container: the {} project to create it in",
                            t.name
                        ),
                    ))
                }
                None => String::new(),
            };
            (t, container, None)
        }
    };
    if !trackers::creates_items(&t.provider) {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            format!("fleet does not create tickets in {} yet", t.name),
        ));
    }
    if !trackers::write_back::writes_creates(&t) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "creating tickets in {} is off; a tracker admin turns it on in Settings → \
                 Trackers → Write-back",
                t.name
            ),
        ));
    }
    let item_org = s.item_org(id)?;
    if crosses(item_org, t.org_id) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{label} belongs to organisation {} and {} to another; fleet does not file \
                 work across organisations",
                item_org.unwrap_or_default(),
                t.name
            ),
        )
        .with_details(serde_json::json!({ "cross_org": true })));
    }
    let signature = fleet_signature(&s, person)?;
    let notes = item.notes.as_deref().map(str::trim).unwrap_or_default();
    let body = if notes.is_empty() {
        signature
    } else {
        format!("{notes}\n\n{signature}")
    };
    let url = crate::store::item_write_url(id);
    let queued = s.enqueue_promotion(&NewTrackerWrite {
        tracker_id: t.id,
        item_key: &item_key,
        op: crate::store::WRITE_OP_CREATE_ITEM,
        url: &url,
        title: item.title.trim(),
        link_id: None,
        claude_session_id: None,
        session_org_id: item_org,
        body: Some(&body),
        external_id: parent_external_id.as_deref(),
    })?;
    if !queued {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{label} is already being created at its tracker"),
        ));
    }
    s.emit_comment_write(&url)?;
    let row = s
        .get_work_item(id)?
        .ok_or_else(|| orgs::not_found("work item", id))?;
    drop(s);
    trackers::write_back::nudge();
    Ok(row)
}

#[cfg(test)]
#[path = "promote_tests.rs"]
mod tests;
