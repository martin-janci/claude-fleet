//! The rows the offline `work_link` benchmark reads (Jev evaluation, test
//! map card J1, phase 0: `fleet-hub decide bench work-link`). Read only;
//! the texts never leave the process except into a local file the operator
//! asked for (`--export-unlinked`, D39).

use super::{ItemMeta, Store};
use crate::ipc_error::IpcError;

/// Link sources that mean a PERSON decided: the link was made by hand, or by
/// starting work on the ticket. `agent`, `agent_inferred` and every
/// resolver/detection source are not a person's decision (D34).
pub const BENCH_PERSON_SOURCES: &[&str] = &["manual", "started"];

/// A confirmed link a person decided, with the prompt that opened the
/// conversation it was decided in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchLinkRow {
    pub link_id: i64,
    pub item_id: i64,
    /// `manual` | `started`.
    pub source: String,
    /// `COALESCE(decided_at, created_at)`.
    pub decided_at: i64,
    pub session_id: i64,
    pub host_alias: String,
    pub first_prompt: String,
    /// The session's `last_prompt` (what fleet last typed there), for the
    /// loop guard that tells fleet's own prompts apart.
    pub last_prompt: Option<String>,
    /// The session's branch: its current one, its worktree's, or the one the
    /// ended link kept.
    pub branch: Option<String>,
    /// The item's tracker's org, else the session's.
    pub org_id: Option<i64>,
}

/// A work item as the benchmark's candidate pool sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchItemRow {
    pub id: i64,
    /// `local`, or the tracker's source word.
    pub source: String,
    pub tracker_id: Option<i64>,
    pub key: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub assignee_id: Option<String>,
    pub status_category: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub unavailable_at: Option<i64>,
}

/// A conversation of a session that has no confirmed or suggested link at
/// all: a D39 hand-label candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchUnlinkedRow {
    pub session_id: i64,
    pub started_at: i64,
    pub host_alias: String,
    pub first_prompt: String,
    pub last_prompt: Option<String>,
    pub branch: Option<String>,
    pub org_id: Option<i64>,
}

/// A confirmed item link with the host it ran on: the host fence of the
/// nudge's candidate set (`tickets.rs` `allowed()`), as of a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchHostLink {
    pub link_id: i64,
    pub item_id: i64,
    pub created_at: i64,
}

impl Store {
    /// Confirmed links a person decided (sources [`BENCH_PERSON_SOURCES`])
    /// at or after `since`, naming an item, whose conversation has a first
    /// prompt — the newest `limit`, returned oldest first. Joined like
    /// [`Store::nl_census_pairs`].
    pub fn bench_work_link_cases(
        &self,
        since: i64,
        limit: u32,
    ) -> Result<Vec<BenchLinkRow>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT l.id, l.item_id, l.source, COALESCE(l.decided_at, l.created_at), \
                    s.id, s.host_alias, c.first_prompt, s.last_prompt, \
                    COALESCE(s.current_branch, \
                             (SELECT wt.branch FROM worktrees wt WHERE wt.id = s.worktree_id), \
                             l.snap_branch), \
                    COALESCE(t.org_id, ",
            crate::session_org_sql!("s"),
            ") FROM work_links l \
             JOIN participants p ON p.id = l.participant_id \
             JOIN sessions s ON s.id = p.session_id \
             JOIN conversations c ON c.session_id = s.id \
                                 AND c.claude_session_id = l.claude_session_id \
             JOIN work_items w ON w.id = l.item_id \
             LEFT JOIN trackers t ON t.id = w.tracker_id \
             WHERE l.state = 'confirmed' AND l.source IN ('manual', 'started') \
               AND c.first_prompt IS NOT NULL \
               AND COALESCE(l.decided_at, l.created_at) >= ?1 \
             ORDER BY COALESCE(l.decided_at, l.created_at) DESC, l.id DESC LIMIT ?2"
        ))?;
        let mut rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                Ok(BenchLinkRow {
                    link_id: r.get(0)?,
                    item_id: r.get(1)?,
                    source: r.get(2)?,
                    decided_at: r.get(3)?,
                    session_id: r.get(4)?,
                    host_alias: r.get(5)?,
                    first_prompt: r.get(6)?,
                    last_prompt: r.get(7)?,
                    branch: r.get(8)?,
                    org_id: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Every work item, with its cached description and assignee.
    pub fn bench_work_items(&self) -> Result<Vec<BenchItemRow>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT id, source, tracker_id, key, title, meta, status_category, \
                    created_at, updated_at, unavailable_at \
             FROM work_items ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                let meta = ItemMeta::parse(r.get::<_, Option<String>>(5)?.as_deref());
                Ok(BenchItemRow {
                    id: r.get(0)?,
                    source: r.get(1)?,
                    tracker_id: r.get(2)?,
                    key: r.get(3)?,
                    title: r.get(4)?,
                    description: meta.description,
                    assignee_id: meta.assignee_id,
                    status_category: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    unavailable_at: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// `(item, org)` for every local item and every org a session that
    /// linked it (any state) belongs to: a local item belongs to no org, so
    /// "local items linked in that org" is how the benchmark scopes them.
    pub fn bench_local_item_orgs(&self) -> Result<Vec<(i64, Option<i64>)>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT DISTINCT l.item_id, COALESCE(l.snap_org_id, ",
            crate::session_org_sql!("s"),
            ") FROM work_links l \
             JOIN work_items w ON w.id = l.item_id AND w.source = 'local' \
             LEFT JOIN participants p ON p.id = l.participant_id \
             LEFT JOIN sessions s ON s.id = p.session_id"
        ))?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Confirmed item links that ran on `host` (live, or ended there): the
    /// rows behind [`Store::work_item_ids_on_host`], with their times.
    pub fn bench_host_links(&self, host: &str) -> Result<Vec<BenchHostLink>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT l.id, l.item_id, l.created_at FROM work_links l \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.item_id IS NOT NULL AND l.state = 'confirmed' AND \
               ((l.ended_at IS NULL AND s.host_alias = ?1) OR \
                (l.ended_at IS NOT NULL AND l.snap_host = ?1))",
        )?;
        let rows = st
            .query_map(rusqlite::params![host], |r| {
                Ok(BenchHostLink {
                    link_id: r.get(0)?,
                    item_id: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Conversations started at or after `since` with a first prompt, of
    /// sessions with NO confirmed or suggested link (live or ended) — the
    /// newest `limit`, returned oldest first.
    pub fn bench_unlinked_conversations(
        &self,
        since: i64,
        limit: u32,
    ) -> Result<Vec<BenchUnlinkedRow>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT s.id, c.started_at, s.host_alias, c.first_prompt, s.last_prompt, \
                    COALESCE(s.current_branch, \
                             (SELECT wt.branch FROM worktrees wt WHERE wt.id = s.worktree_id)), ",
            crate::session_org_sql!("s"),
            " FROM conversations c JOIN sessions s ON s.id = c.session_id \
             WHERE c.first_prompt IS NOT NULL AND c.started_at >= ?1 \
               AND NOT EXISTS (SELECT 1 FROM work_links l \
                                JOIN participants p ON p.id = l.participant_id \
                               WHERE p.session_id = s.id \
                                 AND l.state IN ('confirmed', 'suggested')) \
             ORDER BY c.started_at DESC, c.id DESC LIMIT ?2"
        ))?;
        let mut rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                Ok(BenchUnlinkedRow {
                    session_id: r.get(0)?,
                    started_at: r.get(1)?,
                    host_alias: r.get(2)?,
                    first_prompt: r.get(3)?,
                    last_prompt: r.get(4)?,
                    branch: r.get(5)?,
                    org_id: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }
}
