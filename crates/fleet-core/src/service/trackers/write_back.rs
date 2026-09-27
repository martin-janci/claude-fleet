//! Tracker write-back (work graph M13.4e, decision D3 = yes): the PR remote
//! link, and nothing else — no transition, no worklog, no comment.
//!
//! The sync tick runs [`run`] at the end of each tracker's pass, and it is
//! the ONLY caller: no tool triggers a write, so there is no new MCP action,
//! and nothing a per-host token does can cause one (a link whose latest
//! decision was a per-host token's is never a candidate — see
//! `store::write_outbox`). The pass:
//!
//! 1. does nothing unless the tracker's admin opted in
//!    (`settings.pr_remote_link`, Jira Cloud / Data Center only, off by
//!    default) — not even queue;
//! 2. queues every candidate link's PR (`INSERT OR IGNORE`: a repeated
//!    trigger is a no-op);
//! 3. sends at most [`WRITE_MAX_PER_PASS`] due rows, each re-checked
//!    against this pass's candidates first: a row whose link no longer
//!    allows the write (unlinked, re-decided by a per-host token, moved to
//!    another org) is `cancelled`, never sent;
//! 4. settles each: `done` (plus a `write_back` journal row), or retried
//!    with a backoff, `failed` after [`MAX_ATTEMPTS`]. A 429 honours
//!    `Retry-After`, is not counted as an attempt, and ends the pass's
//!    writes; so does any error that says the tracker as a whole is in
//!    trouble.
//!
//! A write never changes the tracker's `state`: reads decide that.

use super::{TrackerError, TrackerProvider, WriteOp};
use crate::ipc_error::lock;
use crate::store::{
    pr_global_id, RemoteLinkCandidate, Store, TrackerRow, WriteOutboxCounts, OP_PR_REMOTE_LINK,
    REMOTE_LINK_PROVIDERS,
};
use std::sync::Mutex;

/// Rows sent per tracker per pass.
pub const WRITE_MAX_PER_PASS: usize = 20;
/// Attempts before a row is `failed` (about two hours of backoff).
pub const MAX_ATTEMPTS: i64 = 8;
/// The first retry's wait; doubled per attempt, capped at
/// [`RETRY_MAX_SECS`].
pub const RETRY_BASE_SECS: i64 = 60;
pub const RETRY_MAX_SECS: i64 = 3600;

/// What one pass's write-back did (logs and tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteBackPass {
    pub queued: usize,
    pub written: usize,
    pub retried: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// The tracker opted in to the PR remote link, and its provider takes it.
pub fn enabled(t: &TrackerRow) -> bool {
    t.settings.pr_remote_link && REMOTE_LINK_PROVIDERS.contains(&t.provider.as_str())
}

/// The wait before retry number `attempts` (1-based).
pub fn backoff_secs(attempts: i64) -> i64 {
    let shift = attempts.clamp(1, 16) - 1;
    (RETRY_BASE_SECS << shift).min(RETRY_MAX_SECS)
}

/// One pass of write-back for `t` over `provider`. Best-effort: store
/// errors are logged and end the pass; nothing here fails the sync.
pub async fn run(
    t: &TrackerRow,
    provider: &dyn TrackerProvider,
    store: &Mutex<Store>,
    now: i64,
) -> WriteBackPass {
    let mut pass = WriteBackPass::default();
    if !enabled(t) {
        return pass;
    }
    match run_inner(t, provider, store, now, &mut pass).await {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!(tracker_id = t.id, error = %e.message, "[work] tracker write-back stopped");
        }
    }
    if pass != WriteBackPass::default() {
        tracing::info!(
            tracker_id = t.id,
            queued = pass.queued,
            written = pass.written,
            retried = pass.retried,
            failed = pass.failed,
            cancelled = pass.cancelled,
            "[work] tracker write-back"
        );
    }
    pass
}

async fn run_inner(
    t: &TrackerRow,
    provider: &dyn TrackerProvider,
    store: &Mutex<Store>,
    now: i64,
    pass: &mut WriteBackPass,
) -> Result<(), crate::ipc_error::IpcError> {
    let (candidates, due) = {
        let s = lock(store)?;
        let candidates = s.pr_remote_link_candidates(t.id, t.org_id, now)?;
        for c in &candidates {
            if pr_global_id(&c.pr_url).chars().count() > super::jira_common::GLOBAL_ID_MAX_CHARS {
                tracing::debug!(link_id = c.link_id, "[work] PR URL too long for a globalId");
                continue;
            }
            if s.enqueue_pr_remote_link(t.id, c, now)? {
                pass.queued += 1;
            }
        }
        let due = s.due_outbox_writes(t.id, now, WRITE_MAX_PER_PASS)?;
        (candidates, due)
    };
    for row in due {
        let allowed: Option<&RemoteLinkCandidate> = candidates.iter().find(|c| {
            Some(c.link_id) == row.link_id
                && c.issue_id == row.issue_id
                && pr_global_id(&c.pr_url) == row.global_id
        });
        let Some(cand) = allowed.filter(|_| row.op == OP_PR_REMOTE_LINK) else {
            lock(store)?.settle_outbox_write(
                row.id,
                "cancelled",
                Some("the work link no longer allows this write"),
                false,
            )?;
            pass.cancelled += 1;
            continue;
        };
        let op = WriteOp::PrRemoteLink {
            issue_id: row.issue_id.clone(),
            global_id: row.global_id.clone(),
            url: row.url.clone(),
            title: row.title.clone(),
        };
        // The HTTP exchange runs with no lock held.
        let result = provider.write(&op).await;
        let s = lock(store)?;
        let key = row.issue_key.as_deref().unwrap_or(&row.issue_id);
        match result {
            Ok(()) => {
                s.settle_outbox_write(row.id, "done", None, true)?;
                pass.written += 1;
                journal(
                    &s,
                    cand,
                    &format!("PR linked on {key} (remote link): {}", row.url),
                );
            }
            Err(TrackerError::RateLimited { retry_after_secs }) => {
                let wait = retry_after_secs
                    .map(|w| w as i64)
                    .unwrap_or(RETRY_BASE_SECS)
                    .clamp(1, RETRY_MAX_SECS);
                let msg = s.mask_tracker_error(
                    t.id,
                    &TrackerError::RateLimited { retry_after_secs }.explain(),
                )?;
                // The tracker said "later": not an attempt, and no more
                // writes to it this pass.
                s.retry_outbox_write(row.id, now + wait, &msg, false)?;
                pass.retried += 1;
                break;
            }
            Err(e) => {
                let msg = s.mask_tracker_error(t.id, &e.explain())?;
                if row.attempts + 1 >= MAX_ATTEMPTS {
                    s.settle_outbox_write(row.id, "failed", Some(&msg), true)?;
                    pass.failed += 1;
                    journal(&s, cand, &format!("PR not linked on {key}; gave up: {msg}"));
                } else {
                    s.retry_outbox_write(row.id, now + backoff_secs(row.attempts + 1), &msg, true)?;
                    pass.retried += 1;
                }
                // A tracker-wide failure (auth, unreachable …): stop here.
                if e.state().is_some() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Per tracker that opted in or has outbox rows: rows per state and the
/// newest failure (`work_admin { status }`'s `write_back`; master-only).
pub fn status(store: &Mutex<Store>) -> Result<Vec<WriteOutboxCounts>, crate::ipc_error::IpcError> {
    let trackers = lock(store)?.list_trackers()?;
    let mut out = Vec::new();
    for t in trackers {
        let c = lock(store)?.outbox_counts(t.id)?;
        if enabled(&t) || c.pending + c.done + c.failed + c.cancelled > 0 {
            out.push(c);
        }
    }
    Ok(out)
}

/// The `write_back` journal row on the link's conversation, when known.
fn journal(s: &Store, c: &RemoteLinkCandidate, body: &str) {
    if let Some(conv) = c.claude_session_id.as_deref() {
        if let Err(e) = s.append_journal(Some(conv), None, "write_back", "fleet", Some(body), None)
        {
            tracing::debug!(error = %e.message, "[work] write_back journal row failed");
        }
    }
}

#[cfg(test)]
#[path = "tests_write_back.rs"]
mod tests;
