//! `work { action: tracker_comments, key }`: a ticket's comments as its
//! tracker holds them, on demand, for the task page's Comments tab beside
//! fleet's own (owner decision 2026-10-11).
//!
//! * **Never on the sync path.** One live read per call, no cache: a tab
//!   opened is one tracker request, nothing is stored, and no frame or
//!   replay ring carries tracker comment text.
//! * **Scope.** Exactly `describe`'s fence (and so `card`'s): the item is
//!   resolved inside the caller's scope first, and an item outside it
//!   answers as an unknown key ([`orgs::not_visible_to`]).
//! * **Capability before the call.** The item's tracker must still exist
//!   and its provider read comments; a local item or a provider without
//!   the capability is `E_UNSUPPORTED`, never an empty success.
//! * **Fenced for an agent, here.** Bodies and author names are third-party
//!   text: a per-host token (`OrgScope::Host`) gets each body fenced as
//!   `describe` fences a description, and each author defused; a person
//!   reads them plain.
//! * **What fleet posted is not shown twice.** A comment fleet posted
//!   (the outbox's `comment` op) whose fleet comment is live is left out:
//!   the page already shows it as fleet's own, with its post state.
//! * **Never across an `.await`.** The store guard is dropped before the
//!   provider call and re-taken after it.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::trackers::{self, ItemRef, TrackerComment, TrackerNet};
use crate::store::{Store, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// `work { action: tracker_comments, key }`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerComments {
    /// The tracker's own key, when it has one (else the reference asked).
    pub key: String,
    /// The tracker's name, as Settings shows it.
    pub tracker: String,
    /// Its provider (`jira`, `jira_dc`, `github`, `linear`, `asana`).
    pub provider: String,
    /// The newest [`trackers::TRACKER_COMMENTS_MAX`], oldest first, without
    /// the ones fleet posted from a live comment.
    pub comments: Vec<TrackerComment>,
    /// The caller may ask for a fleet comment to be posted here too: the
    /// tracker's admin turned `write_back.comments` on and the caller is a
    /// person (a per-host token never posts).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_post: bool,
}

/// The key a ticket's outbox rows carry: its key, else its tracker id (the
/// reference of a provider without keys).
pub fn outbox_key(item: &WorkItemRow) -> Option<String> {
    item.key.clone().or_else(|| item.external_id.clone())
}

/// Whether `scope` may ask for a post: a person, never a per-host token.
pub fn may_post(scope: &OrgScope) -> bool {
    !matches!(scope, OrgScope::Host { .. })
}

fn unsupported(key: &str) -> IpcError {
    IpcError::new(
        codes::E_UNSUPPORTED,
        format!("{key} has no tracker whose comments fleet reads; open the ticket"),
    )
}

/// The reference a provider reads an item's comments by: Jira by key (its
/// comment links name the key), the rest by their own id.
fn item_ref(provider: &str, key: Option<&str>, external_id: Option<&str>) -> Option<ItemRef> {
    let by_key = key.map(ItemRef::parse);
    let by_id = external_id
        .filter(|e| !e.is_empty())
        .map(|e| ItemRef::Id(e.to_string()));
    if matches!(provider, "jira" | "jira_dc") {
        by_key.or(by_id)
    } else {
        by_id.or(by_key)
    }
}

/// `work { action: tracker_comments, key }`. `net` is the caller's tracker
/// network (`trackers::default_net()` in production, a fake in tests).
pub async fn tracker_comments(
    store: &Mutex<Store>,
    scope: &OrgScope,
    key: &str,
    net: &TrackerNet,
) -> Result<TrackerComments, IpcError> {
    let key = crate::store::normalize_work_ref(key)?;
    let (out_key, item, tracker) = {
        let s = lock(store)?;
        orgs::require_key(&s, scope, &key)?;
        let Some(item) = orgs::visible_item_for_key(&s, scope, &key)? else {
            return Err(orgs::not_visible_to(scope, &key));
        };
        let tracker = match item.tracker_id {
            Some(id) => s.get_tracker(id)?,
            None => None,
        };
        let out_key = item.key.clone().unwrap_or_else(|| key.clone());
        (out_key, item, tracker)
    };
    let Some(t) = tracker.filter(|t| trackers::provider_caps(t).comments) else {
        return Err(unsupported(&out_key));
    };
    let Some(r) = item_ref(
        &t.provider,
        item.key.as_deref(),
        item.external_id.as_deref(),
    ) else {
        return Err(unsupported(&out_key));
    };
    let (cred, hidden) = {
        let s = lock(store)?;
        let hidden = match outbox_key(&item) {
            Some(k) => s.posted_comment_ids(t.id, &k)?,
            None => Default::default(),
        };
        (s.resolve_tracker_credential(t.id)?, hidden)
    };
    if cred.is_none() && trackers::needs_credential(&t) {
        return Err(trackers::TrackerError::Unconfigured.to_ipc());
    }
    let provider = trackers::provider_for(&t, cred, net).map_err(|e| e.to_ipc())?;
    let comments = provider
        .comments(&r, trackers::TRACKER_COMMENTS_MAX)
        .await
        .map_err(|e| e.to_ipc())?
        .into_iter()
        .filter(|c| !hidden.contains(&c.id))
        .map(|c| fence_for_scope(scope, c))
        .collect();
    Ok(TrackerComments {
        key: out_key,
        tracker: t.name.clone(),
        provider: t.provider.clone(),
        comments,
        can_post: may_post(scope) && trackers::write_back::writes_comments(&t),
    })
}

/// A comment as `scope` reads it: for a per-host token the body fenced as
/// `describe` fences a description and the author defused; a person reads
/// both plain.
fn fence_for_scope(scope: &OrgScope, mut c: TrackerComment) -> TrackerComment {
    if let OrgScope::Host { .. } = scope {
        c.body = crate::mcp::guard::fence_untrusted(
            &c.body,
            "a tracker ticket's comment",
            trackers::TRACKER_COMMENT_MAX_CHARS,
        );
        c.author = crate::mcp::guard::defuse(&c.author);
    }
    c
}

#[cfg(test)]
#[path = "tracker_comments_tests.rs"]
mod tests;
