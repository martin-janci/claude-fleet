//! The tracker write outbox (migration 061, work graph M13.4e, decision
//! D3): the PR remote link, and nothing else. See the migration for the
//! table; the rules that matter here:
//!
//! * **Who may cause a write.** Only a live (or just-ended, within
//!   [`ENDED_GRACE_SECS`]) `confirmed` link whose source is `manual` or
//!   `started` — a person's decision — never a suggestion, an inference, a
//!   fork, a resume or an inheritance; and never a link whose latest
//!   decision came from a per-host token (`host_decided`).
//! * **Where to.** Only the tracker of the link's own org: the session's
//!   org (live) or the org it ended in (`snap_org_id`) must equal the
//!   tracker's, unassigned included — never across orgs, `force_cross_org`
//!   or not.
//! * **Idempotent.** One row per (tracker, issue, op, `globalId`): a
//!   repeated trigger is `INSERT OR IGNORE`, and Jira upserts by `globalId`
//!   besides.
//!
//! What the sync tick does with the rows is `service::trackers::write_back`.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

/// `op` of a PR remote link row.
pub const OP_PR_REMOTE_LINK: &str = "pr_remote_link";

/// `state` values: `pending` is retried by the sync tick; the rest are
/// settled and swept by retention.
pub const OUTBOX_STATES: &[&str] = &["pending", "done", "failed", "cancelled"];

/// How long after a link ended its PR still gets its remote link (a session
/// that opened a PR and was killed before the next sync).
pub const ENDED_GRACE_SECS: i64 = 86_400;

/// Link sources whose PR may be written: a person's decision only.
pub const WRITE_LINK_SOURCES: &[&str] = &["manual", "started"];

/// Longest PR URL taken (the sync's `URL_MAX_CHARS`).
pub const PR_URL_MAX_CHARS: usize = 2048;

/// One link whose PR may go to the tracker now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLinkCandidate {
    pub link_id: i64,
    /// The issue's tracker id (Jira's numeric id).
    pub issue_id: String,
    pub issue_key: Option<String>,
    pub pr_url: String,
    /// The conversation the `write_back` journal row goes to.
    pub claude_session_id: Option<String>,
}

/// One outbox row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WriteOutboxRow {
    pub id: i64,
    pub tracker_id: i64,
    pub issue_id: String,
    #[serde(default)]
    pub issue_key: Option<String>,
    #[serde(default)]
    pub link_id: Option<i64>,
    pub op: String,
    pub global_id: String,
    pub url: String,
    pub title: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Outbox rows per state, for one tracker (`work_admin { status }`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WriteOutboxCounts {
    pub tracker_id: i64,
    #[serde(default)]
    pub pending: i64,
    #[serde(default)]
    pub done: i64,
    #[serde(default)]
    pub failed: i64,
    #[serde(default)]
    pub cancelled: i64,
    /// The newest failure's reason (already redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

const OUTBOX_COLUMNS: &str = "id, tracker_id, issue_id, issue_key, link_id, op, global_id, url, \
     title, state, attempts, next_attempt_at, last_error, created_at, updated_at";

fn map_outbox(r: &rusqlite::Row<'_>) -> rusqlite::Result<WriteOutboxRow> {
    Ok(WriteOutboxRow {
        id: r.get(0)?,
        tracker_id: r.get(1)?,
        issue_id: r.get(2)?,
        issue_key: r.get(3)?,
        link_id: r.get(4)?,
        op: r.get(5)?,
        global_id: r.get(6)?,
        url: r.get(7)?,
        title: r.get(8)?,
        state: r.get(9)?,
        attempts: r.get(10)?,
        next_attempt_at: r.get(11)?,
        last_error: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

/// A PR URL fleet will hand to a tracker: `https://`, one line, bounded.
pub fn valid_pr_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() > "https://".len()
        && url.chars().count() <= PR_URL_MAX_CHARS
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The remote link's `globalId` for a PR (the M9.5 plan's form).
pub fn pr_global_id(url: &str) -> String {
    format!("fleet:pr:{url}")
}

/// The remote link's title: fleet's own words, never third-party text.
/// `https://github.com/o/r/pull/12` → `Pull request o/r#12`.
pub fn pr_title(url: &str) -> String {
    let path = url
        .trim_start_matches("https://")
        .split_once('/')
        .map(|(_, p)| p)
        .unwrap_or("");
    let parts: Vec<&str> = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .split('/')
        .collect();
    match parts.as_slice() {
        [owner, repo, "pull" | "pulls" | "merge_requests", n, ..]
            if !owner.is_empty() && !repo.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("Pull request {owner}/{repo}#{n}")
        }
        _ => "Pull request".into(),
    }
}

impl Store {
    /// The links whose PR may be written to tracker `tracker_id` (org
    /// `tracker_org`) now. See the module docs for every rule.
    pub fn pr_remote_link_candidates(
        &self,
        tracker_id: i64,
        tracker_org: Option<i64>,
        now: i64,
    ) -> Result<Vec<RemoteLinkCandidate>, IpcError> {
        let sources = WRITE_LINK_SOURCES
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = Vec::new();
        // Live links: the session's current PR, the session's org.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT l.id, i.external_id, i.key, s.pr_url, s.id, s.claude_session_id \
             FROM work_links l \
             JOIN work_items i ON i.id = l.item_id \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             JOIN sessions s ON s.id = p.session_id \
             WHERE i.tracker_id = ?1 AND i.external_id IS NOT NULL \
               AND l.ended_at IS NULL AND l.state = 'confirmed' AND l.host_decided = 0 \
               AND l.source IN ({sources}) AND s.pr_url IS NOT NULL \
             ORDER BY l.id"
        ))?;
        let live: Vec<(RemoteLinkCandidate, i64)> = stmt
            .query_map(rusqlite::params![tracker_id], |r| {
                Ok((
                    RemoteLinkCandidate {
                        link_id: r.get(0)?,
                        issue_id: r.get(1)?,
                        issue_key: r.get(2)?,
                        pr_url: r.get(3)?,
                        claude_session_id: r.get(5)?,
                    },
                    r.get(4)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (c, sid) in live {
            if valid_pr_url(&c.pr_url) && self.session_org(sid)? == tracker_org {
                out.push(c);
            }
        }
        // Links that ended within the grace: the PR and org they ended with.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT l.id, i.external_id, i.key, l.snap_pr_url, l.snap_org_id, l.snap_claude_ids \
             FROM work_links l \
             JOIN work_items i ON i.id = l.item_id \
             WHERE i.tracker_id = ?1 AND i.external_id IS NOT NULL \
               AND l.ended_at IS NOT NULL AND l.ended_at >= ?2 \
               AND l.state = 'confirmed' AND l.host_decided = 0 \
               AND l.source IN ({sources}) AND l.snap_pr_url IS NOT NULL \
             ORDER BY l.id"
        ))?;
        let ended: Vec<(RemoteLinkCandidate, Option<i64>)> = stmt
            .query_map(rusqlite::params![tracker_id, now - ENDED_GRACE_SECS], |r| {
                let ids: Option<String> = r.get(5)?;
                let last = ids
                    .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
                    .and_then(|v| v.last().cloned());
                Ok((
                    RemoteLinkCandidate {
                        link_id: r.get(0)?,
                        issue_id: r.get(1)?,
                        issue_key: r.get(2)?,
                        pr_url: r.get(3)?,
                        claude_session_id: last,
                    },
                    r.get(4)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (c, org) in ended {
            if valid_pr_url(&c.pr_url) && org == tracker_org {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Queue `c`'s PR remote link for `tracker_id`. `false` when the same
    /// (issue, `globalId`) is already queued or written: a no-op.
    pub fn enqueue_pr_remote_link(
        &self,
        tracker_id: i64,
        c: &RemoteLinkCandidate,
        now: i64,
    ) -> Result<bool, IpcError> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO tracker_write_outbox \
               (tracker_id, issue_id, issue_key, link_id, op, global_id, url, title, state, \
                attempts, next_attempt_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, ?9, ?9, ?9)",
            rusqlite::params![
                tracker_id,
                c.issue_id,
                c.issue_key,
                c.link_id,
                OP_PR_REMOTE_LINK,
                pr_global_id(&c.pr_url),
                c.pr_url,
                pr_title(&c.pr_url),
                now
            ],
        )? > 0)
    }

    /// Pending rows of `tracker_id` due at `now`, oldest first.
    pub fn due_outbox_writes(
        &self,
        tracker_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<WriteOutboxRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {OUTBOX_COLUMNS} FROM tracker_write_outbox \
             WHERE tracker_id = ?1 AND state = 'pending' AND next_attempt_at <= ?2 \
             ORDER BY next_attempt_at, id LIMIT ?3"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![tracker_id, now, limit as i64], map_outbox)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Every row of `tracker_id`, oldest first (tests, diagnostics).
    pub fn outbox_rows(&self, tracker_id: i64) -> Result<Vec<WriteOutboxRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {OUTBOX_COLUMNS} FROM tracker_write_outbox WHERE tracker_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![tracker_id], map_outbox)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Settle row `id`: `done`, `failed` or `cancelled`. `attempted` counts
    /// this pass's attempt.
    pub fn settle_outbox_write(
        &self,
        id: i64,
        state: &str,
        error: Option<&str>,
        attempted: bool,
    ) -> Result<bool, IpcError> {
        debug_assert!(OUTBOX_STATES.contains(&state) && state != "pending");
        Ok(self.conn.execute(
            "UPDATE tracker_write_outbox SET state = ?2, last_error = ?3, \
               attempts = attempts + ?4, updated_at = ?5 \
             WHERE id = ?1 AND state = 'pending'",
            rusqlite::params![id, state, error, attempted as i64, now_unix()],
        )? > 0)
    }

    /// Keep row `id` pending until `next_at`. `attempted` counts this
    /// pass's attempt (a rate limit is not one: the tracker said "later").
    pub fn retry_outbox_write(
        &self,
        id: i64,
        next_at: i64,
        error: &str,
        attempted: bool,
    ) -> Result<bool, IpcError> {
        Ok(self.conn.execute(
            "UPDATE tracker_write_outbox SET next_attempt_at = ?2, last_error = ?3, \
               attempts = attempts + ?4, updated_at = ?5 \
             WHERE id = ?1 AND state = 'pending'",
            rusqlite::params![id, next_at, error, attempted as i64, now_unix()],
        )? > 0)
    }

    /// Rows per state of `tracker_id`, and the newest failure's reason.
    pub fn outbox_counts(&self, tracker_id: i64) -> Result<WriteOutboxCounts, IpcError> {
        let mut c = WriteOutboxCounts {
            tracker_id,
            ..Default::default()
        };
        let mut stmt = self.conn.prepare(
            "SELECT state, COUNT(*) FROM tracker_write_outbox WHERE tracker_id = ?1 GROUP BY state",
        )?;
        let rows: Vec<(String, i64)> = stmt
            .query_map(rusqlite::params![tracker_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        for (state, n) in rows {
            match state.as_str() {
                "pending" => c.pending = n,
                "done" => c.done = n,
                "failed" => c.failed = n,
                "cancelled" => c.cancelled = n,
                _ => {}
            }
        }
        c.last_error = self
            .conn
            .query_row(
                "SELECT last_error FROM tracker_write_outbox \
                 WHERE tracker_id = ?1 AND last_error IS NOT NULL AND state != 'done' \
                 ORDER BY updated_at DESC, id DESC LIMIT 1",
                rusqlite::params![tracker_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(c)
    }
}

#[cfg(test)]
#[path = "write_outbox/tests.rs"]
mod tests;
