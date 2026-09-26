//! The self-cleaning lifecycle's storage (work graph M7.2, migration 050):
//! what the tidy planner reads, the UI-only archive of a live session, snooze
//! and never per link, the last touch, and reopened work.
//!
//! Nothing here deletes a row. Archive, snooze and never are flags on the
//! live link; a reopen is a stamp on the item plus a `reopened` journal row.
//! A per-session keep (work graph M11.3) is a `tidy_kept` timeline event
//! whose detail is the unix second it holds until: no column, no migration.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use crate::service::gc::tidy::{TidyLink, TidySession};
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};

/// A work item that moved out of `done` and has past sessions (the
/// Attention "Reopened" entry).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReopenedWork {
    pub item_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub reopened_at: i64,
    /// Ended confirmed links to it: its past sessions.
    #[serde(default)]
    pub past_sessions: u32,
    /// Live sessions still linked to it.
    #[serde(default)]
    pub live_sessions: u32,
    /// The host of the newest past session, for the preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_host: Option<String>,
    /// The item's org: its tracker's (work graph M5); `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
}

/// `sessions` columns only the tidy planner reads: the last touch, the PR
/// signals, the live branch.
type SessionExtra = (Option<i64>, Option<String>, Option<String>);

/// The timeline event a per-session keep writes (work graph M11.3).
pub const EVENT_TIDY_KEPT: &str = "tidy_kept";

impl Store {
    /// Everything [`crate::service::gc::tidy::plan_tidy`] reads, one entry
    /// per session row (live and ghost). A few whole-table reads, no N+1.
    pub fn tidy_sessions(&self) -> Result<Vec<TidySession>, IpcError> {
        let rows = self.list_all_sessions()?;
        let mut links: HashMap<i64, TidyLink> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT p.session_id, l.id, COALESCE(i.key, l.ref_key), i.status_category, \
                        i.status_name, i.resolution, i.status_changed_at, l.archived_at, \
                        l.tidy_snoozed_until, l.tidy_never, \
                        (SELECT t.org_id FROM trackers t WHERE t.id = i.tracker_id) \
                 FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 LEFT JOIN work_items i ON i.id = l.item_id \
                 WHERE l.ended_at IS NULL AND l.is_primary = 1 AND l.state = 'confirmed' \
                   AND p.session_id IS NOT NULL",
            )?;
            let it = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TidyLink {
                        link_id: r.get(1)?,
                        key: r.get(2)?,
                        status_category: r.get(3)?,
                        status_name: r.get(4)?,
                        resolution: r.get(5)?,
                        status_changed_at: r.get(6)?,
                        archived_at: r.get(7)?,
                        snoozed_until: r.get(8)?,
                        never: r.get::<_, i64>(9)? != 0,
                        org_id: r.get(10)?,
                    },
                ))
            })?;
            for row in it {
                let (sid, l) = row?;
                links.insert(sid, l);
            }
        }
        // ANY live confirmed link to an in-progress item protects a session,
        // not only its primary.
        let in_progress: HashSet<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 JOIN work_items i ON i.id = l.item_id \
                 WHERE l.ended_at IS NULL AND l.state = 'confirmed' \
                   AND i.status_category = 'in_progress' AND p.session_id IS NOT NULL",
            )?;
            let it = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            it.collect::<rusqlite::Result<_>>()?
        };
        // A snooze / never on ANY live confirmed link (the flags are per
        // link, and `work_link { snooze | never, link_id }` takes a
        // secondary one): the planner honours the latest snooze and any never.
        let mut flags: HashMap<i64, (Option<i64>, bool)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT p.session_id, MAX(l.tidy_snoozed_until), MAX(l.tidy_never) \
                 FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.ended_at IS NULL AND l.state = 'confirmed' AND p.session_id IS NOT NULL \
                 GROUP BY p.session_id",
            )?;
            let it = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (r.get(1)?, r.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0),
                ))
            })?;
            for row in it {
                let (sid, f) = row?;
                flags.insert(sid, f);
            }
        }
        let mut extra: HashMap<i64, SessionExtra> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, last_touch_at, pr_signals, current_branch FROM sessions")?;
            let it = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (r.get(1)?, r.get::<_, Option<String>>(2)?, r.get(3)?),
                ))
            })?;
            for row in it {
                let (id, e) = row?;
                extra.insert(id, e);
            }
        }
        // Any live link but a rejection — a non-primary confirmed link or a
        // suggestion no one decided — ties a session to work
        // (`TidySession::any_link`, the `idle_unlinked` gate).
        let any_link: HashSet<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.ended_at IS NULL AND l.state <> 'rejected' AND p.session_id IS NOT NULL",
            )?;
            let it = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            it.collect::<rusqlite::Result<_>>()?
        };
        // The latest keep of each session (a later, shorter keep replaces a
        // longer one), written after the row was created: `sessions.id` can
        // be reused, and a keep never outlives the session it was given to.
        // Driven from `sessions`: each row's keep is found through the
        // `(session_id, at)` index, never a walk of all of `session_events`.
        let kept: HashMap<i64, i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT s.id, CAST(e.detail AS INTEGER) FROM sessions s \
                 JOIN session_events e ON e.id = \
                   (SELECT MAX(x.id) FROM session_events x \
                    WHERE x.session_id = s.id AND x.kind = ?1) \
                 WHERE e.at >= s.created_at",
            )?;
            let it = stmt.query_map([EVENT_TIDY_KEPT], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            let mut out = HashMap::new();
            for row in it {
                if let (id, Some(until)) = row? {
                    out.insert(id, until);
                }
            }
            out
        };
        let tasks: HashSet<i64> = self
            .open_tasks()?
            .into_iter()
            .flat_map(|t| [t.worker_session_id, t.requester_session_id])
            .flatten()
            .collect();
        Ok(rows
            .into_iter()
            .map(|row| {
                let (touch, signals, branch) = extra.remove(&row.id).unwrap_or_default();
                let pr_merged = signals
                    .as_deref()
                    .and_then(|s| {
                        serde_json::from_str::<crate::service::work::detect::PrSignals>(s).ok()
                    })
                    .is_some_and(|s| s.is_merged());
                let (snoozed_until, never) = flags.remove(&row.id).unwrap_or_default();
                TidySession {
                    link: links.remove(&row.id),
                    in_progress: in_progress.contains(&row.id),
                    snoozed_until,
                    never,
                    pr_merged,
                    last_touch_at: touch,
                    open_tasks: tasks.contains(&row.id),
                    branch: branch.or_else(|| row.worktree_key.clone()),
                    any_link: any_link.contains(&row.id),
                    kept_until: kept.get(&row.id).copied(),
                    row,
                }
            })
            .collect())
    }

    /// Keep a session out of tidy-up until `until` (work graph M11.3): a
    /// `tidy_kept` timeline event the planner reads. The latest keep wins.
    pub fn keep_tidy(&self, session_id: i64, until: i64) -> Result<(), IpcError> {
        self.insert_session_event(session_id, EVENT_TIDY_KEPT, Some(&until.to_string()))
    }

    /// The live participant of a session row, or `E_NOTFOUND`.
    fn live_participant(&self, session_id: i64) -> Result<i64, IpcError> {
        self.conn
            .query_row(
                "SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
            })
    }

    /// Archive a live session from the UI: its live confirmed links get
    /// `archived_at`, so it collapses into its work group's Done section.
    /// tmux keeps running. `E_INVALID` for a session with no linked work
    /// (there is nothing to collapse it into). Idempotent: an archived link
    /// keeps its first stamp. Emits the row.
    pub fn archive_session_work(&self, session_id: i64) -> Result<usize, IpcError> {
        self.archive_session_links(session_id, None)
    }

    /// [`Self::archive_session_work`] restricted to `only` of the session's
    /// live confirmed links (a per-host token's visible ones, work graph
    /// M5): a link outside the list is never stamped, and a session whose
    /// linked work is all outside it answers as one with no linked work.
    pub fn archive_session_links(
        &self,
        session_id: i64,
        only: Option<&[i64]>,
    ) -> Result<usize, IpcError> {
        let participant = self.live_participant(session_id)?;
        let only_json = only
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        let linked: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
               AND ended_at IS NULL AND state = 'confirmed' \
               AND (?2 IS NULL OR id IN (SELECT value FROM json_each(?2))))",
            rusqlite::params![participant, only_json],
            |r| r.get(0),
        )?;
        if !linked {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("session {session_id} has no linked work to archive under"),
            ));
        }
        let n = self.conn.execute(
            "UPDATE work_links SET archived_at = ?2 WHERE participant_id = ?1 \
               AND ended_at IS NULL AND state = 'confirmed' AND archived_at IS NULL \
               AND (?3 IS NULL OR id IN (SELECT value FROM json_each(?3)))",
            rusqlite::params![participant, now_unix(), only_json],
        )?;
        if n > 0 {
            self.bump_row_for_lifecycle(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(n)
    }

    /// Un-archive a live session (one click, or any touch). Returns how many
    /// links were un-archived; emits the row when any was.
    pub fn unarchive_session_work(&self, session_id: i64) -> Result<usize, IpcError> {
        let n = self.conn.execute(
            "UPDATE work_links SET archived_at = NULL WHERE archived_at IS NOT NULL \
               AND ended_at IS NULL AND participant_id = \
                 (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL)",
            rusqlite::params![session_id],
        )?;
        if n > 0 {
            self.bump_row_for_lifecycle(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(n)
    }

    /// A person is using the session (a prompt, or an attach): stamp
    /// `last_touch_at` (the tidy planner's one-hour protection) and
    /// un-archive it. Emits the row only when it was archived. `false` for a
    /// row that does not exist.
    pub fn touch_session(&self, session_id: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE sessions SET last_touch_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        if n == 0 {
            return Ok(false);
        }
        self.unarchive_session_work(session_id)?;
        Ok(true)
    }

    /// [`Self::touch_session`] by tmux name (a prompt sent through fleet).
    pub fn touch_session_by_name(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE host_alias = ?1 AND tmux_name = ?2",
                rusqlite::params![host_alias, tmux_name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.touch_session(id)?;
        }
        Ok(())
    }

    /// Stamp `last_touch_at` without emitting: the caller emits the row
    /// itself (the UserPromptSubmit hook's write). Un-archives too.
    pub(super) fn touch_for_prompt(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET last_touch_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        self.conn.execute(
            "UPDATE work_links SET archived_at = NULL WHERE archived_at IS NOT NULL \
               AND ended_at IS NULL AND participant_id = \
                 (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL)",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    fn bump_row_for_lifecycle(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }

    /// The live confirmed link a tidy flag is written to: `link_id` when
    /// given (it must be the session's), else the session's primary. The
    /// service resolves it first so a per-host token's visibility check
    /// covers the primary too (work graph M5).
    pub fn tidy_link(&self, session_id: i64, link_id: Option<i64>) -> Result<i64, IpcError> {
        let participant = self.live_participant(session_id)?;
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM work_links WHERE participant_id = ?1 AND ended_at IS NULL \
                   AND state = 'confirmed' AND (?2 IS NULL AND is_primary = 1 OR id = ?2) \
                 ORDER BY is_primary DESC, id LIMIT 1",
                rusqlite::params![participant, link_id],
                |r| r.get(0),
            )
            .optional()?;
        found.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                match link_id {
                    Some(l) => format!("session {session_id} has no live work link {l}"),
                    None => format!("session {session_id} has no linked work"),
                },
            )
        })
    }

    /// "Snooze": Tidy-up does not suggest the session before `until`.
    /// Idempotent; a later snooze replaces an earlier one. Returns the link.
    pub fn snooze_tidy(
        &self,
        session_id: i64,
        link_id: Option<i64>,
        until: i64,
    ) -> Result<i64, IpcError> {
        let id = self.tidy_link(session_id, link_id)?;
        self.conn.execute(
            "UPDATE work_links SET tidy_snoozed_until = ?2 WHERE id = ?1",
            rusqlite::params![id, until],
        )?;
        Ok(id)
    }

    /// "Never for this work": Tidy-up never suggests the session while this
    /// link is live. Idempotent. Returns the link.
    pub fn never_tidy(&self, session_id: i64, link_id: Option<i64>) -> Result<i64, IpcError> {
        let id = self.tidy_link(session_id, link_id)?;
        self.conn.execute(
            "UPDATE work_links SET tidy_never = 1 WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(id)
    }

    /// A tracker moved `item_id` out of `done` (`from` → `to`, status
    /// names): stamp `reopened_at` and write a `reopened` journal row — on
    /// the conversation of every live session working on it, else on the
    /// last conversation of its newest past session. The event is the
    /// transition, never a guess from the current state.
    pub(crate) fn record_reopened(
        &self,
        item_id: i64,
        key: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<usize, IpcError> {
        let now = now_unix();
        self.conn.execute(
            "UPDATE work_items SET reopened_at = ?2 WHERE id = ?1",
            rusqlite::params![item_id, now],
        )?;
        let mut targets: Vec<(String, Option<i64>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT s.claude_session_id, p.id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 JOIN sessions s ON s.id = p.session_id \
                 WHERE l.item_id = ?1 AND l.ended_at IS NULL AND l.state = 'confirmed' \
                   AND s.claude_session_id IS NOT NULL",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![item_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if targets.is_empty() {
            let last: Option<String> = self
                .conn
                .query_row(
                    "SELECT (SELECT value FROM json_each(l.snap_claude_ids) \
                             ORDER BY key DESC LIMIT 1) \
                     FROM work_links l WHERE l.item_id = ?1 AND l.ended_at IS NOT NULL \
                       AND l.state = 'confirmed' AND l.snap_claude_ids IS NOT NULL \
                     ORDER BY l.ended_at DESC, l.id DESC LIMIT 1",
                    rusqlite::params![item_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            targets.extend(last.map(|c| (c, None)));
        }
        let body = format!("{}: {from} → {to} (reopened)", key.unwrap_or("item"));
        let meta = serde_json::json!({
            "item_id": item_id, "from": from, "to": to, "reopened": true
        })
        .to_string();
        let mut n = 0;
        for (claude, participant) in targets {
            if self
                .append_journal(
                    Some(&claude),
                    participant,
                    "reopened",
                    "fleet",
                    Some(&body),
                    Some(&meta),
                )?
                .is_some()
            {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Work that is open again and has past sessions, newest reopen first.
    /// An item drops out when it is done again, when it is dismissed, or when
    /// a session links to it after the reopen (the work was resumed).
    pub fn reopened_work(&self) -> Result<Vec<ReopenedWork>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT i.id, i.key, i.title, i.status_name, i.url, i.reopened_at, \
                    (SELECT COUNT(*) FROM work_links l WHERE l.item_id = i.id \
                       AND l.ended_at IS NOT NULL AND l.state = 'confirmed') AS past, \
                    (SELECT COUNT(*) FROM work_links l WHERE l.item_id = i.id \
                       AND l.ended_at IS NULL AND l.state = 'confirmed') AS live, \
                    (SELECT l.snap_host FROM work_links l WHERE l.item_id = i.id \
                       AND l.ended_at IS NOT NULL AND l.state = 'confirmed' \
                     ORDER BY l.ended_at DESC, l.id DESC LIMIT 1), \
                    (SELECT t.org_id FROM trackers t WHERE t.id = i.tracker_id) \
             FROM work_items i \
             WHERE i.reopened_at IS NOT NULL AND i.status_category <> 'done' \
               AND NOT EXISTS (SELECT 1 FROM work_links l WHERE l.item_id = i.id \
                                 AND l.ended_at IS NULL AND l.state = 'confirmed' \
                                 AND l.created_at >= i.reopened_at) \
               AND past > 0 \
             ORDER BY i.reopened_at DESC, i.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ReopenedWork {
                item_id: r.get(0)?,
                key: r.get(1)?,
                title: r.get(2)?,
                status_name: r.get(3)?,
                url: r.get(4)?,
                reopened_at: r.get(5)?,
                past_sessions: r.get::<_, i64>(6)?.max(0) as u32,
                live_sessions: r.get::<_, i64>(7)?.max(0) as u32,
                last_host: r.get(8)?,
                org_id: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Dismiss the Attention "Reopened" entry of `item_id`. `false` when it
    /// was not reopened.
    pub fn dismiss_reopened(&self, item_id: i64) -> Result<bool, IpcError> {
        if self.get_work_item(item_id)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {item_id} not found"),
            ));
        }
        Ok(self.conn.execute(
            "UPDATE work_items SET reopened_at = NULL WHERE id = ?1 AND reopened_at IS NOT NULL",
            rusqlite::params![item_id],
        )? > 0)
    }
}

#[cfg(test)]
mod tests;
