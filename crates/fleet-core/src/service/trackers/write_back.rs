//! Write-back to trackers (work graph M13.4e, decisions D3 / D29): the PR
//! remote link, and nothing else.
//!
//! When the PR probe sees a pull request on a session, [`on_pr`] queues one
//! write per item the session is linked to — only where all of these hold:
//!
//! * the link is **confirmed** and a person made it (`manual` / `started`,
//!   [`PERSON_SOURCES`]), never a detection guess, an agent's inference, or
//!   an agent's link or start (`agent` / `agent_started`, D34);
//! * the item belongs to a Jira tracker (Cloud or Data Center) whose admin
//!   turned on `write_back.pr_remote_link` (off by default);
//! * the session's org is the tracker's org (both known and different
//!   refuses; there is no `force_cross_org` for a write);
//! * the PR URL has the `https://<host>/<owner>/<repo>/pull/<n>` shape.
//!
//! The outbox (migration 061) makes a repeat a no-op, and the sync pass
//! drains it ([`drain`]) with the credential it already holds, re-checking
//! the setting and the org first. The remote link's global id is the PR's
//! URL, so Jira upserts it: even a write that is sent twice adds one link.
//! Nothing a transcript or a tracker wrote is ever sent: only the URL and
//! fleet's own title.
//!
//! No caller triggers a write: the toggle is `work_admin` (master only), and
//! the trigger is the PR probe. A per-host token can do neither.

use super::{TrackerError, TrackerProvider, WriteOp};
use crate::ipc_error::{lock, IpcError};
use crate::store::{NewTrackerWrite, Store, TrackerRow, PERSON_SOURCES, WRITE_OP_PR_REMOTE_LINK};
use std::sync::Mutex;

/// Writes one pass sends per tracker, at most.
pub const DRAIN_BATCH: usize = 20;

/// The longest a failed write waits before its next try.
const MAX_BACKOFF_SECS: i64 = 6 * 3_600;

/// PURE: fleet's title for a PR URL, `PR: owner/repo#n`, or `None` when the
/// URL is not a pull request's (`https://<host>/<owner>/<repo>/pull/<n>`,
/// nothing after the number but an optional `/`).
pub fn pr_title(url: &str) -> Option<String> {
    if url.len() > 2_048 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').collect();
    let [host, owner, repo, "pull", n] = parts.as_slice() else {
        return None;
    };
    let name = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            && p != "."
            && p != ".."
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    let n_ok = !n.is_empty() && n.len() <= 12 && n.chars().all(|c| c.is_ascii_digit());
    (host_ok && name(owner) && name(repo) && n_ok).then(|| format!("PR: {owner}/{repo}#{n}"))
}

/// A session gained (or re-signalled) the pull request `pr_url`: queue its
/// remote link on each item it may be written to. Returns how many writes
/// were newly queued. Best-effort: the caller logs an error and goes on.
pub fn on_pr(s: &Store, session_id: i64, pr_url: &str) -> Result<usize, IpcError> {
    let Some(title) = pr_title(pr_url) else {
        return Ok(0);
    };
    let Some(participant) = s.participant_for_session(session_id)? else {
        return Ok(0);
    };
    let session_org = s.session_org(session_id)?;
    let claude_session_id = s
        .get_session_by_id(session_id)?
        .and_then(|r| r.claude_session_id);
    let mut queued = 0;
    for (link, _) in s.detection_links(participant.id)? {
        if link.state != "confirmed" || !PERSON_SOURCES.contains(&link.source.as_str()) {
            continue;
        }
        let Some(item_id) = link.item_id else {
            continue;
        };
        let Some(item) = s.get_work_item(item_id)? else {
            continue;
        };
        let (Some(tracker_id), Some(key)) = (item.tracker_id, item.key.as_deref()) else {
            continue;
        };
        let Some(t) = s.get_tracker(tracker_id)? else {
            continue;
        };
        if !writes_pr_links(&t) || crosses(session_org, t.org_id) {
            continue;
        }
        if s.enqueue_tracker_write(&NewTrackerWrite {
            tracker_id,
            item_key: key,
            op: WRITE_OP_PR_REMOTE_LINK,
            url: pr_url,
            title: &title,
            link_id: Some(link.id),
            claude_session_id: claude_session_id.as_deref(),
            session_org_id: session_org,
        })? {
            queued += 1;
        }
    }
    Ok(queued)
}

/// The tracker is a Jira whose admin turned the PR remote link on.
fn writes_pr_links(t: &TrackerRow) -> bool {
    matches!(t.provider.as_str(), "jira" | "jira_dc") && t.settings.write_back.pr_remote_link
}

/// Both orgs known and different: never write there.
fn crosses(session_org: Option<i64>, tracker_org: Option<i64>) -> bool {
    matches!((session_org, tracker_org), (Some(a), Some(b)) if a != b)
}

/// PURE: how long a write waits after its `attempts`-th failure: a minute,
/// doubling, at most [`MAX_BACKOFF_SECS`].
pub fn backoff_secs(attempts: i64) -> i64 {
    let shift = attempts.clamp(0, 16) as u32;
    (60_i64 << shift).min(MAX_BACKOFF_SECS)
}

/// What one drain did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub sent: usize,
    pub retrying: usize,
    pub given_up: usize,
}

/// Send `t`'s due writes through `provider` (the sync pass's own). Every
/// write re-checks the setting and the org; a rate limit stops the drain
/// and spends no attempt. Settled rows older than the journal's retention
/// window are dropped at the end. Errors are per row: nothing here fails
/// the read pass.
pub async fn drain(
    t: &TrackerRow,
    provider: &dyn TrackerProvider,
    store: &Mutex<Store>,
    now: i64,
) -> DrainReport {
    let mut report = DrainReport::default();
    let due = match lock(store).and_then(|s| s.due_tracker_writes(t.id, now, DRAIN_BATCH)) {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(tracker = t.id, error = %e.message, "[write-back] outbox unreadable");
            return report;
        }
    };
    for w in due {
        let settle = |r: Result<(), IpcError>| {
            if let Err(e) = r {
                tracing::debug!(write = w.id, error = %e.message, "[write-back] not settled");
            }
        };
        // Re-checked at send time: the admin may have turned it off, or
        // moved the tracker to another org, since the write was queued.
        if !writes_pr_links(t) {
            continue;
        }
        if crosses(w.session_org_id, t.org_id) {
            settle(lock(store).and_then(|s| {
                s.retry_tracker_write(w.id, "the session's org is not this tracker's", None, true)
            }));
            report.given_up += 1;
            continue;
        }
        let op = WriteOp::PrRemoteLink {
            key: w.item_key.clone(),
            url: w.url.clone(),
            title: w.title.clone(),
        };
        match provider.write(&op).await {
            Ok(()) => {
                settle(lock(store).and_then(|s| {
                    s.finish_tracker_write(w.id)?;
                    if let Some(cid) = w.claude_session_id.as_deref() {
                        let body = format!("Added {} to {} as a remote link", w.url, w.item_key);
                        let meta =
                            serde_json::json!({ "tracker_id": t.id, "op": w.op }).to_string();
                        s.append_journal(
                            Some(cid),
                            None,
                            "write_back",
                            "fleet",
                            Some(&body),
                            Some(&meta),
                        )?;
                    }
                    Ok(())
                }));
                report.sent += 1;
            }
            Err(TrackerError::RateLimited { retry_after_secs }) => {
                let wait = retry_after_secs.map_or(60, |s| s as i64).max(1);
                settle(lock(store).and_then(|s| {
                    s.retry_tracker_write(w.id, "rate limited", Some(now + wait), false)
                }));
                report.retrying += 1;
                // The rest would meet the same limit.
                break;
            }
            Err(e) => {
                let msg = crate::service::trackers::sync::metric_error(&e.explain());
                // A refusal no retry can fix gives up now; anything that
                // may heal (a credential, the network) backs off.
                let next = match e {
                    TrackerError::Forbidden(_)
                    | TrackerError::NotFound
                    | TrackerError::Invalid(_) => None,
                    _ => Some(now + backoff_secs(w.attempts)),
                };
                if next.is_none() || w.attempts + 1 >= crate::store::WRITE_MAX_ATTEMPTS {
                    report.given_up += 1;
                } else {
                    report.retrying += 1;
                }
                settle(lock(store).and_then(|s| s.retry_tracker_write(w.id, &msg, next, true)));
            }
        }
    }
    let _ = lock(store).and_then(|s| {
        let days = crate::service::work::retention::RetentionDays::from_store(&s).journal;
        if days > 0 {
            s.sweep_tracker_writes(now - days * 86_400)?;
        }
        Ok(())
    });
    report
}

#[cfg(test)]
mod tests;
