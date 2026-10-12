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
use crate::service::trackers::{self, ItemRef, TrackerAttachment, TrackerComment, TrackerNet};
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
    /// The ticket's attachments, oldest first, when its provider lists them
    /// (`caps.attachments`). A failed listing leaves this empty rather than
    /// failing the comments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<TrackerAttachment>,
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
    let attachments = if trackers::provider_caps(&t).attachments {
        match provider
            .attachments(&r, trackers::TRACKER_ATTACHMENTS_MAX)
            .await
        {
            Ok(a) => a.into_iter().map(|a| defuse_for_scope(scope, a)).collect(),
            Err(e) => {
                tracing::debug!(key = %out_key, "[work] attachments not listed: {e:?}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    Ok(TrackerComments {
        key: out_key,
        tracker: t.name.clone(),
        provider: t.provider.clone(),
        comments,
        attachments,
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

/// Most of a start brief the ticket's comments and attachments take.
pub const BRIEF_EXTRAS_MAX_CHARS: usize = 1800;
/// Newest comments a start brief carries.
pub const BRIEF_COMMENTS_MAX: usize = 10;
/// Longest single comment a start brief carries, in characters.
pub const BRIEF_COMMENT_MAX_CHARS: usize = 600;
/// Attachments a start brief names.
pub const BRIEF_ATTACHMENTS_MAX: usize = 15;
/// How long a start waits for the tracker before briefing without them.
pub const BRIEF_EXTRAS_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

/// The tail of a ticket start's brief: the ticket's attachments (name and
/// link) and newest comments, read live from its tracker by the hub, so
/// the session's Claude needs no tracker credential of its own (owner
/// report 2026-10-11: an Asana session had nothing but the key). Third-
/// party text, all inside one fence. Empty when the plan has no tracker
/// item, the provider reads neither, there is no credential, the tracker
/// fails or does not answer within [`BRIEF_EXTRAS_WAIT`]: the start is
/// never failed over it. The caller has already checked the ticket's text
/// may reach the plan's host (`tickets::brief_visible_on`).
pub async fn brief_extras(
    store: &Mutex<Store>,
    plan: &crate::service::trackers::tickets::StartPlan,
    net: &TrackerNet,
) -> String {
    match tokio::time::timeout(BRIEF_EXTRAS_WAIT, read_extras(store, plan, net)).await {
        Ok(Some((attachments, comments))) => brief_tail(&attachments, &comments),
        Ok(None) => String::new(),
        Err(_) => {
            tracing::info!(key = %plan.key, "[work] tracker too slow; briefing without its comments");
            String::new()
        }
    }
}

async fn read_extras(
    store: &Mutex<Store>,
    plan: &crate::service::trackers::tickets::StartPlan,
    net: &TrackerNet,
) -> Option<(Vec<TrackerAttachment>, Vec<TrackerComment>)> {
    let (item, t, cred, hidden) = {
        let s = lock(store).ok()?;
        let item = s.get_work_item(plan.item_id?).ok()??;
        let t = s.get_tracker(item.tracker_id?).ok()??;
        let hidden = match outbox_key(&item) {
            Some(k) => s.posted_comment_ids(t.id, &k).ok()?,
            None => Default::default(),
        };
        let cred = s.resolve_tracker_credential(t.id).ok()?;
        (item, t, cred, hidden)
    };
    let caps = trackers::provider_caps(&t);
    if !caps.comments && !caps.attachments {
        return None;
    }
    if cred.is_none() && trackers::needs_credential(&t) {
        return None;
    }
    let r = item_ref(
        &t.provider,
        item.key.as_deref(),
        item.external_id.as_deref(),
    )?;
    let provider = trackers::provider_for(&t, cred, net).ok()?;
    let attachments = if caps.attachments {
        provider
            .attachments(&r, BRIEF_ATTACHMENTS_MAX)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let comments = if caps.comments {
        provider
            .comments(&r, BRIEF_COMMENTS_MAX)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !hidden.contains(&c.id))
            .collect()
    } else {
        Vec::new()
    };
    Some((attachments, comments))
}

/// PURE: the brief's tail for these attachments and comments (oldest
/// first, as the providers answer), at most [`BRIEF_EXTRAS_MAX_CHARS`].
/// The newest comments are the ones kept when they do not all fit; a cut
/// never loses the fence's end.
pub fn brief_tail(attachments: &[TrackerAttachment], comments: &[TrackerComment]) -> String {
    if attachments.is_empty() && comments.is_empty() {
        return String::new();
    }
    const FROM: &str = "the tracker ticket's attachments and comments";
    let overhead = crate::mcp::guard::fence_untrusted("", FROM, 0)
        .chars()
        .count()
        + 80;
    let budget = BRIEF_EXTRAS_MAX_CHARS.saturating_sub(overhead);
    let mut body = String::new();
    if !attachments.is_empty() {
        body.push_str("Attachments (open with the tracker's sign-in):\n");
        for a in attachments {
            let line = match &a.url {
                Some(u) => format!("- {}: {u}\n", a.name),
                None => format!("- {}\n", a.name),
            };
            if body.chars().count() + line.chars().count() > budget / 2 {
                body.push_str("- (more on the ticket)\n");
                break;
            }
            body.push_str(&line);
        }
    }
    // Newest first until the budget is spent, then put back in order.
    let mut kept: Vec<String> = Vec::new();
    let mut used = body.chars().count();
    for c in comments.iter().rev() {
        let text: String = c
            .body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(BRIEF_COMMENT_MAX_CHARS)
            .collect();
        let day = crate::service::usage::day_string(c.created_at.div_euclid(86_400));
        let entry = format!("{} ({day}): {text}\n", c.author);
        if used + entry.chars().count() + 40 > budget {
            break;
        }
        used += entry.chars().count();
        kept.push(entry);
    }
    if !kept.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        let older = comments.len() - kept.len();
        body.push_str(&if older > 0 {
            format!("Latest comments ({older} older on the ticket):\n")
        } else {
            "Comments:\n".to_string()
        });
        for e in kept.iter().rev() {
            body.push_str(e);
        }
    }
    let fenced = crate::mcp::guard::fence_untrusted(body.trim_end(), FROM, budget);
    format!("\nFrom the ticket, read live by fleet:\n{fenced}\n")
}

/// An attachment as `scope` reads it: for a per-host token the name is
/// defused (it is the uploader's text); a person reads it plain.
fn defuse_for_scope(scope: &OrgScope, mut a: TrackerAttachment) -> TrackerAttachment {
    if let OrgScope::Host { .. } = scope {
        a.name = crate::mcp::guard::defuse(&a.name);
    }
    a
}

#[cfg(test)]
#[path = "tracker_comments_tests.rs"]
mod tests;
