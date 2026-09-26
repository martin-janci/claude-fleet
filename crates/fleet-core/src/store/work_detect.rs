//! Work detection storage (work graph M4, migration 049): the state signals
//! a session carries (`current_branch`, `pr_signals`), the resolver's view
//! of its live links, and applying the resolver's changes. The rules are in
//! `service::work::resolve`; this file only reads and writes.

use super::{now_unix, Store, WorkLinkRow};
use crate::ipc_error::{codes, IpcError};
use crate::service::work::resolve::{Evidence, LinkChange, NewState, PrimaryRef, EVIDENCE_MAX};
use rusqlite::OptionalExtension;

/// Longest stored branch name.
const BRANCH_MAX_CHARS: usize = 255;

/// What the resolver needs to know about a session besides its links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectionState {
    pub session_id: i64,
    pub participant: i64,
    pub claude_session_id: Option<String>,
    pub project_id: Option<i64>,
    /// GitHub `owner/repo` of the project, when it is on GitHub.
    pub repo: Option<String>,
    /// The live branch (transcript `gitBranch`), else the worktree's branch.
    pub branch: Option<String>,
    /// `sessions.pr_signals` (JSON), `None` when never probed or no PR.
    pub pr_signals: Option<String>,
    /// The probe has run for this session at least once.
    pub pr_probed: bool,
    /// The last prompt fleet itself sent (the loop guard).
    pub last_prompt: Option<String>,
}

impl Store {
    /// Store the session's live branch. `true` when it changed (the caller
    /// then resolves). Empty or `HEAD` (detached) reads as no branch.
    pub fn set_current_branch(&self, session_id: i64, branch: &str) -> Result<bool, IpcError> {
        let b: String = branch.trim().chars().take(BRANCH_MAX_CHARS).collect();
        let b = (!b.is_empty() && b != "HEAD").then_some(b);
        let n = self.conn.execute(
            "UPDATE sessions SET current_branch = ?1, current_branch_at = ?2 \
             WHERE id = ?3 AND current_branch IS NOT ?1",
            rusqlite::params![b, now_unix(), session_id],
        )?;
        Ok(n > 0)
    }

    /// Store what the PR probe read for `tmux_name` on `host` (`None`: no
    /// PR). Returns the session id when the value changed.
    pub fn set_pr_signals(
        &self,
        host: &str,
        tmux_name: &str,
        signals: Option<&str>,
    ) -> Result<Option<i64>, IpcError> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE host_alias = ?1 AND tmux_name = ?2",
                rusqlite::params![host, tmux_name],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        let n = self.conn.execute(
            "UPDATE sessions SET pr_signals = ?1, pr_signals_at = ?2 \
             WHERE id = ?3 AND (pr_signals IS NOT ?1 OR pr_signals_at IS NULL)",
            rusqlite::params![signals, now_unix(), id],
        )?;
        Ok((n > 0).then_some(id))
    }

    /// The resolver's view of `session_id`, `None` for a missing row.
    pub fn detection_state(&self, session_id: i64) -> Result<Option<DetectionState>, IpcError> {
        let row = self
            .conn
            .query_row(
                "SELECT s.claude_session_id, s.project_id, \
                        CASE WHEN p.owner IS NOT NULL AND p.owner <> 'local' \
                             THEN p.owner || '/' || p.repo END, s.pr_url, \
                        COALESCE(s.current_branch, w.branch), s.pr_signals, \
                        s.pr_signals_at IS NOT NULL, s.last_prompt \
                 FROM sessions s \
                 LEFT JOIN projects p ON p.id = s.project_id \
                 LEFT JOIN worktrees w ON w.id = s.worktree_id \
                 WHERE s.id = ?1",
                rusqlite::params![session_id],
                |r| {
                    Ok(DetectionState {
                        session_id,
                        participant: 0,
                        claude_session_id: r.get(0)?,
                        project_id: r.get(1)?,
                        repo: r
                            .get::<_, Option<String>>(2)?
                            .or_else(|| repo_of_pr_url(r.get::<_, Option<String>>(3).ok()??)),
                        branch: r.get(4)?,
                        pr_signals: r.get(5)?,
                        pr_probed: r.get(6)?,
                        last_prompt: r.get(7)?,
                    })
                },
            )
            .optional()?;
        let Some(mut st) = row else { return Ok(None) };
        st.participant = self.ensure_participant_for_session(session_id)?;
        Ok(Some(st))
    }

    /// Every live link of `participant` with its target: the item's key, the
    /// link's `ref_key`, or `item:<id>` for a keyless local item.
    pub fn detection_links(
        &self,
        participant: i64,
    ) -> Result<Vec<(WorkLinkRow, String)>, IpcError> {
        let cols = super::work::link_columns_prefixed("l");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols}, COALESCE(i.key, l.ref_key, 'item:' || l.item_id) \
             FROM work_links l LEFT JOIN work_items i ON i.id = l.item_id \
             WHERE l.participant_id = ?1 AND l.ended_at IS NULL ORDER BY l.id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![participant], |r| {
            Ok((
                super::work::map_link(r)?,
                r.get::<_, String>(super::work::LINK_COLUMN_COUNT)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The item's CURRENT key for a detected target that names a tracker
    /// item by an alias (a moved Jira issue), else the target as given. The
    /// resolver keys links and candidates by one spelling, so a candidate
    /// seen under an alias meets the link (or the rejection, R9) made for
    /// the item, instead of being created again on every run.
    pub fn canonical_work_target(
        &self,
        target: &str,
        tracker_id: Option<i64>,
    ) -> Result<String, IpcError> {
        let Ok(key) = super::normalize_work_ref(target) else {
            return Ok(target.to_string());
        };
        let current: Option<String> = match tracker_id {
            Some(tid) => self
                .conn
                .query_row(
                    "SELECT key FROM work_items WHERE tracker_id = ?1 AND (key = ?2 OR EXISTS \
                       (SELECT 1 FROM json_each(COALESCE(aliases, '[]')) WHERE value = ?2)) \
                     ORDER BY (key = ?2) DESC, id LIMIT 1",
                    rusqlite::params![tid, key],
                    |r| r.get(0),
                )
                .optional()?
                .flatten(),
            None => self.tracker_item_for_key(&key)?.and_then(|i| i.key),
        };
        Ok(current
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| target.to_string()))
    }

    /// Delivered handover bodies addressed to `participant`, newest first
    /// (the loop guard: text fleet injected must not count as evidence).
    pub fn recent_handover_bodies(
        &self,
        participant: i64,
        limit: i64,
    ) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT body FROM work_journal WHERE kind = 'handover' AND participant_id = ?1 \
               AND body IS NOT NULL ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![participant, limit], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Apply the resolver's changes for one session, in one transaction.
    /// Bumps the row and emits `session_updated` when anything changed.
    /// Returns whether it did.
    ///
    /// Runs under a SAVEPOINT (Task 3), so it works standalone (autocommit)
    /// and nested inside `Store::atomically` — `service::work::detect::on_prompt`
    /// / `resolve_session` reach this from the UserPromptSubmit and
    /// SessionStart hooks' own transactions.
    pub fn apply_link_changes(
        &self,
        session_id: i64,
        participant: i64,
        conversation: Option<&str>,
        changes: &[LinkChange],
    ) -> Result<bool, IpcError> {
        if changes.is_empty() {
            return Ok(false);
        }
        let now = now_unix();
        // Work graph M5: a guess never crosses orgs. Detection neither
        // proposes nor auto-links another org's item on this session; a
        // person can, with `force_cross_org`.
        let session_org = self.session_org(session_id)?;
        let crosses = |item: Option<i64>| -> Result<bool, IpcError> {
            let Some(i) = item else { return Ok(false) };
            Ok(matches!((self.item_org(i)?, session_org), (Some(a), Some(b)) if a != b))
        };
        // `Store::in_savepoint`: one commit standalone, a nested savepoint
        // inside a hook's or reconcile's transaction.
        self.in_savepoint("apply_link_changes", |_| -> Result<(), IpcError> {
            let mut created: Vec<(String, i64)> = Vec::new();
            for c in changes {
                match c {
                    LinkChange::Create {
                        target,
                        state,
                        source,
                        strength,
                        rule,
                        preselected,
                        tracker_id,
                        evidence,
                    } => {
                        let (item_id, ref_key) = self.detected_target(target, *tracker_id)?;
                        if crosses(item_id)? {
                            continue;
                        }
                        // One live link per item and participant, however the
                        // target was spelled: a second one would sit beside a
                        // decision (R1 / R9) or double a suggestion.
                        if let Some(item) = item_id {
                            let dup: bool = self.conn.query_row(
                                "SELECT EXISTS(SELECT 1 FROM work_links WHERE participant_id = ?1 \
                                   AND ended_at IS NULL AND item_id = ?2)",
                                rusqlite::params![participant, item],
                                |r| r.get(0),
                            )?;
                            if dup {
                                continue;
                            }
                        }
                        let state = match state {
                            NewState::Suggested => "suggested",
                            NewState::Confirmed => "confirmed",
                        };
                        let decided = (state == "confirmed").then_some(now);
                        self.conn.execute(
                            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                               is_primary, created_at, decided_at, claude_session_id, strength, rule, \
                               evidence, preselected) \
                             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                            rusqlite::params![
                                item_id,
                                ref_key,
                                participant,
                                state,
                                source,
                                now,
                                decided,
                                conversation,
                                strength.as_str(),
                                rule,
                                encode_evidence(&[], evidence),
                                *preselected as i64
                            ],
                        )?;
                        created.push((target.clone(), self.conn.last_insert_rowid()));
                    }
                    LinkChange::End { link_id, reason } => {
                        self.end_live_link(*link_id, session_id, reason, now)?;
                    }
                    LinkChange::Withdraw { link_id } | LinkChange::Decay { link_id } => {
                        self.conn.execute(
                            "DELETE FROM work_links WHERE id = ?1 AND state = 'suggested' \
                               AND ended_at IS NULL",
                            rusqlite::params![link_id],
                        )?;
                    }
                    LinkChange::Promote {
                        link_id,
                        rule,
                        source,
                        strength,
                        evidence,
                    } => {
                        let item: Option<i64> = self
                            .conn
                            .query_row(
                                "SELECT item_id FROM work_links WHERE id = ?1",
                                rusqlite::params![link_id],
                                |r| r.get(0),
                            )
                            .optional()?
                            .flatten();
                        if crosses(item)? {
                            continue;
                        }
                        let old = self.link_evidence(*link_id)?;
                        // The link is now the promoting signal's (R7 ends a
                        // `branch` / `pr` link when that signal moves on).
                        self.conn.execute(
                            "UPDATE work_links SET state = 'confirmed', rule = ?2, strength = ?3, \
                               evidence = ?4, decided_at = ?5, claude_session_id = ?6, \
                               source = ?7, preselected = 0 \
                             WHERE id = ?1 AND state = 'suggested' AND ended_at IS NULL",
                            rusqlite::params![
                                link_id,
                                rule,
                                strength.as_str(),
                                encode_evidence(&old, evidence),
                                now,
                                conversation,
                                source
                            ],
                        )?;
                    }
                    LinkChange::Touch {
                        link_id,
                        evidence,
                        conversation: conv,
                        strength,
                        preselected,
                    } => {
                        let old = self.link_evidence(*link_id)?;
                        self.conn.execute(
                            "UPDATE work_links SET evidence = ?2, \
                               claude_session_id = CASE WHEN state = 'suggested' \
                                 THEN COALESCE(?3, claude_session_id) ELSE claude_session_id END, \
                               strength = COALESCE(?4, strength), \
                               preselected = MAX(preselected, ?5) \
                             WHERE id = ?1 AND ended_at IS NULL",
                            rusqlite::params![
                                link_id,
                                encode_evidence(&old, evidence),
                                conv,
                                strength.map(|s| s.as_str()),
                                *preselected as i64
                            ],
                        )?;
                    }
                    LinkChange::Primary(r) => {
                        let id = match r {
                            PrimaryRef::Link(id) => Some(*id),
                            PrimaryRef::Target(t) => {
                                created.iter().find(|(ct, _)| ct == t).map(|(_, id)| *id)
                            }
                            PrimaryRef::None => None,
                        };
                        match (r, id) {
                            (PrimaryRef::None, _) => {
                                self.conn.execute(
                                    "UPDATE work_links SET is_primary = 0 \
                                     WHERE participant_id = ?1 AND ended_at IS NULL",
                                    rusqlite::params![participant],
                                )?;
                            }
                            // One guarded statement: the old primary is cleared
                            // only when the new one is a live confirmed link of
                            // this participant. A Promote skipped above (cross-
                            // org) left it suggested, so nothing moves.
                            (_, Some(id)) => {
                                self.conn.execute(
                                    "UPDATE work_links SET is_primary = (id = ?2) \
                                     WHERE participant_id = ?1 AND ended_at IS NULL \
                                       AND EXISTS(SELECT 1 FROM work_links n WHERE n.id = ?2 \
                                           AND n.participant_id = ?1 AND n.state = 'confirmed' \
                                           AND n.ended_at IS NULL)",
                                    rusqlite::params![participant, id],
                                )?;
                            }
                            // The target's Create was skipped (cross-org): the
                            // session keeps the primary it has.
                            (_, None) => {}
                        }
                    }
                }
            }
            self.conn.execute(
                "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                rusqlite::params![session_id],
            )?;
            Ok(())
        })?;
        self.emit_session(session_id)?;
        Ok(true)
    }

    /// `(item_id, ref_key)` for a detected target: the item of the tracker a
    /// URL named, else the one tracker item / local item that has the key,
    /// else a bare reference a later sync binds.
    fn detected_target(
        &self,
        target: &str,
        tracker_id: Option<i64>,
    ) -> Result<(Option<i64>, Option<String>), IpcError> {
        let key = super::normalize_work_ref(target)?;
        if let Some(tid) = tracker_id {
            let item: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM work_items WHERE tracker_id = ?1 AND (key = ?2 OR EXISTS \
                       (SELECT 1 FROM json_each(COALESCE(aliases, '[]')) WHERE value = ?2)) \
                     ORDER BY (key = ?2) DESC, id LIMIT 1",
                    rusqlite::params![tid, key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(item) = item {
                return Ok((Some(item), Some(key)));
            }
        }
        if let Some(id) = target
            .strip_prefix("item:")
            .and_then(|n| n.parse::<i64>().ok())
        {
            return Ok((Some(id), None));
        }
        self.resolve_work_key(&key)
    }

    fn link_evidence(&self, link_id: i64) -> Result<Vec<serde_json::Value>, IpcError> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT evidence FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(raw
            .and_then(|e| serde_json::from_str(&e).ok())
            .unwrap_or_default())
    }

    /// End a live session's link because its state signal moved on (R7):
    /// the same snapshot the retirement trigger takes, plus the reason.
    /// `snap_branch` is the branch the link's own `branch` evidence saw
    /// (the value that just changed), else the worktree's — never a PR
    /// closing ref's text, which is a ticket key.
    fn end_live_link(
        &self,
        link_id: i64,
        session_id: i64,
        reason: &str,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE work_links SET ended_at = ?3, end_reason = ?4, is_primary = 0, \
               snap_host = (SELECT host_alias FROM sessions WHERE id = ?2), \
               snap_tmux = (SELECT tmux_name FROM sessions WHERE id = ?2), \
               snap_name = (SELECT friendly_name FROM sessions WHERE id = ?2), \
               snap_project_id = (SELECT project_id FROM sessions WHERE id = ?2), \
               snap_worktree = (SELECT worktree_key FROM sessions WHERE id = ?2), \
               snap_branch = COALESCE( \
                 (SELECT json_extract(j.value, '$.text') FROM json_each(evidence) j \
                   WHERE json_extract(j.value, '$.signal') = 'branch' \
                   ORDER BY j.key DESC LIMIT 1), \
                 (SELECT w.branch FROM sessions s JOIN worktrees w ON w.id = s.worktree_id \
                   WHERE s.id = ?2)), \
               snap_pr_url = (SELECT pr_url FROM sessions WHERE id = ?2), \
               snap_claude_ids = (SELECT json_group_array(claude_session_id) FROM \
                 (SELECT claude_session_id FROM conversations WHERE session_id = ?2 \
                   ORDER BY started_at) HAVING COUNT(*) > 0) \
             WHERE id = ?1 AND ended_at IS NULL",
            rusqlite::params![link_id, session_id, now, reason],
        )?;
        Ok(())
    }

    /// A person decides one suggestion by id: `confirm` makes it a confirmed
    /// manual link and the session's primary; otherwise it is rejected
    /// (sticky, R9). `E_NOTFOUND` when the link is not this session's live
    /// link.
    pub fn decide_work_link(
        &self,
        session_id: i64,
        link_id: i64,
        confirm: bool,
    ) -> Result<WorkLinkRow, IpcError> {
        let participant: Option<i64> = self
            .conn
            .query_row(
                "SELECT l.participant_id FROM work_links l JOIN participants p \
                   ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.id = ?1 AND l.ended_at IS NULL AND p.session_id = ?2",
                rusqlite::params![link_id, session_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(participant) = participant else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} has no live work link {link_id}"),
            ));
        };
        let conv: Option<String> = self
            .conn
            .query_row(
                "SELECT claude_session_id FROM sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        if confirm {
            self.conn.execute(
                "UPDATE work_links SET is_primary = 0 \
                 WHERE participant_id = ?1 AND ended_at IS NULL",
                rusqlite::params![participant],
            )?;
        }
        // A decision keeps the evidence and the rule that proposed it, so
        // "why" still reads after the person agreed.
        self.conn.execute(
            "UPDATE work_links SET state = ?2, source = 'manual', is_primary = ?3, \
               decided_at = ?4, claude_session_id = COALESCE(?5, claude_session_id), \
               strength = 'explicit', preselected = 0 \
             WHERE id = ?1",
            rusqlite::params![
                link_id,
                if confirm { "confirmed" } else { "rejected" },
                confirm as i64,
                now,
                conv
            ],
        )?;
        self.conn.execute(
            "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        tx.commit()?;
        self.emit_session(session_id)?;
        self.get_work_link(link_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work link vanished after write"))
    }

    /// Fleet already knows `key`: an item carries it (or an alias), or some
    /// link names it. With no tracker, only such keys count from a prompt.
    /// A removed tracker's rows do not count (a link to one still does).
    pub fn work_key_known(&self, key: &str) -> Result<bool, IpcError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_items WHERE key = ?1 \
                           AND (tracker_id IS NULL OR tracker_id IN (SELECT id FROM trackers))) \
                 OR EXISTS(SELECT 1 FROM work_links WHERE ref_key = ?1) \
                 OR EXISTS(SELECT 1 FROM work_items, json_each(COALESCE(aliases, '[]')) j \
                           WHERE j.value = ?1 \
                             AND (tracker_id IS NULL OR tracker_id IN (SELECT id FROM trackers)))",
            rusqlite::params![key],
            |r| r.get(0),
        )?)
    }

    /// Project ids whose sessions have at least `n` branch links a person
    /// confirmed from a suggestion (the auto-trust count): live or ended.
    pub fn confirmed_branch_suggestions(&self, project_id: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM work_links l \
             LEFT JOIN participants p ON p.id = l.participant_id \
             LEFT JOIN sessions s ON s.id = p.session_id \
             WHERE l.state = 'confirmed' AND l.source = 'manual' \
               AND l.rule IN ('R3b', 'R4') \
               AND COALESCE(s.project_id, l.snap_project_id) = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )?)
    }

    /// The detection backlog (work graph M12.4): live suggestions a person
    /// has not decided that were made before `before` (unix seconds), on a
    /// live session — on `host`'s sessions only when one is given. A weak
    /// suggestion beside a confirmed primary is not counted: the session
    /// row's `work_suggested` hides it too, so nobody is asked about it.
    /// Driven from `sessions`, then each session's participant and its live
    /// links, all by index.
    pub fn detection_backlog(&self, before: i64, host: Option<&str>) -> Result<u32, IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions s \
             JOIN participants p ON p.session_id = s.id AND p.retired_at IS NULL \
             JOIN work_links l ON l.participant_id = p.id AND l.ended_at IS NULL \
             WHERE l.state = 'suggested' AND l.created_at < ?1 \
               AND (?2 IS NULL OR s.host_alias = ?2) \
               AND (l.strength IS NOT 'weak' OR NOT EXISTS \
                    (SELECT 1 FROM work_links c \
                      WHERE c.participant_id = p.id AND c.ended_at IS NULL \
                        AND c.is_primary = 1 AND c.state = 'confirmed'))",
            rusqlite::params![before, host],
            |r| r.get(0),
        )?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

/// `owner/repo` of a GitHub PR URL, lower case.
fn repo_of_pr_url(url: String) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    (!o.is_empty() && !r.is_empty() && parts.next() == Some("pull"))
        .then(|| format!("{}/{}", o.to_ascii_lowercase(), r.to_ascii_lowercase()))
}

/// Merge new evidence onto the stored lines, keeping the newest
/// [`EVIDENCE_MAX`].
fn encode_evidence(old: &[serde_json::Value], new: &[Evidence]) -> String {
    let mut all: Vec<serde_json::Value> = old.to_vec();
    all.extend(new.iter().filter_map(|e| serde_json::to_value(e).ok()));
    let skip = all.len().saturating_sub(EVIDENCE_MAX);
    serde_json::to_string(&all[skip..]).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
impl Store {
    /// Test-only: turn a link into `state`, made at `created_at`, not primary.
    pub(crate) fn set_work_link_state_for_test(
        &self,
        id: i64,
        state: &str,
        created_at: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE work_links SET state = ?2, is_primary = 0, created_at = ?3 WHERE id = ?1",
            rusqlite::params![id, state, created_at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkTarget;

    const DAY: i64 = 86_400;

    /// A live link of a new session on `host`, turned into a suggestion made
    /// at `at` of `strength`. Returns (session, link).
    fn suggestion(
        s: &Store,
        name: &str,
        host: &str,
        key: &str,
        at: i64,
        strength: &str,
    ) -> (i64, i64) {
        let sid = s
            .upsert_session(name, host, None, None, 1, 1, "running", None)
            .unwrap();
        let l = s
            .link_session_work(sid, WorkTarget::Key(key), "manual")
            .unwrap();
        s.conn
            .execute(
                "UPDATE work_links SET state = 'suggested', is_primary = 0, \
                 created_at = ?2, strength = ?3 WHERE id = ?1",
                rusqlite::params![l.id, at, strength],
            )
            .unwrap();
        (sid, l.id)
    }

    #[test]
    fn the_detection_backlog_counts_old_undecided_suggestions_on_live_sessions() {
        let s = Store::open_in_memory().unwrap();
        for h in ["h1", "h2"] {
            s.upsert_host(h).unwrap();
        }
        let now = 100 * DAY;
        let cutoff = now - 7 * DAY;
        // Counted: old, strong, on h1 and h2.
        suggestion(&s, "a", "h1", "ABC-1", now - 30 * DAY, "strong");
        suggestion(&s, "b", "h2", "ABC-2", now - 8 * DAY, "weak");
        // Not counted: too recent.
        suggestion(&s, "c", "h1", "ABC-3", now - DAY, "strong");
        // Not counted: decided (confirmed back), ended, on a retired session.
        let (_, decided) = suggestion(&s, "d", "h1", "ABC-4", now - 30 * DAY, "strong");
        s.conn
            .execute(
                "UPDATE work_links SET state = 'confirmed' WHERE id = ?1",
                [decided],
            )
            .unwrap();
        let (_, ended) = suggestion(&s, "e", "h1", "ABC-5", now - 30 * DAY, "strong");
        s.conn
            .execute("UPDATE work_links SET ended_at = 1 WHERE id = ?1", [ended])
            .unwrap();
        let (gone, _) = suggestion(&s, "f", "h1", "ABC-6", now - 30 * DAY, "strong");
        s.conn
            .execute(
                "UPDATE participants SET retired_at = 1 WHERE session_id = ?1",
                [gone],
            )
            .unwrap();
        // Not counted: a weak suggestion beside a confirmed primary (hidden
        // from the row, so nobody is asked).
        let g = s
            .upsert_session("g", "h1", None, None, 1, 1, "running", None)
            .unwrap();
        let primary = s
            .link_session_work(g, WorkTarget::Key("ABC-7"), "manual")
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO work_links (ref_key, participant_id, state, source, created_at, strength) \
                 VALUES ('ABC-8', ?1, 'suggested', 'detected', ?2, 'weak')",
                rusqlite::params![primary.participant_id, now - 30 * DAY],
            )
            .unwrap();

        assert_eq!(s.detection_backlog(cutoff, None).unwrap(), 2);
        assert_eq!(s.detection_backlog(cutoff, Some("h1")).unwrap(), 1);
        assert_eq!(s.detection_backlog(cutoff, Some("h2")).unwrap(), 1);
        assert_eq!(s.detection_backlog(cutoff, Some("h3")).unwrap(), 0);
        assert_eq!(
            s.detection_backlog(now, None).unwrap(),
            3,
            "the recent one too"
        );
    }
}
