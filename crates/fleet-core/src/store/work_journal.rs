//! Work memory (migration 047): durable notes about Claude conversations,
//! harvested from hooks and transcripts so past work can be resumed with its
//! context — even after the session row (and its `conversations`, which
//! cascade with it) is gone. See the migration for the model; the rules that
//! matter here:
//!
//! * Keyed by `claude_session_id`, never by session or link: a work key finds
//!   its journal through its links' conversations (`snap_claude_ids` of an
//!   ended link, the live session's conversations for a live one), so a
//!   relink reassigns history without rewriting it.
//! * Every session is journaled, linked or not, within per-conversation caps
//!   ([`PROGRESS_CAP`], [`COMPACT_SUMMARY_CAP`], one `conversation` row).
//! * A `handover` row is a brief addressed to a session (`participant_id`),
//!   delivered once through a hook's `additionalContext` and stamped
//!   `delivered_at`. It is never evidence for anything (the loop guard).

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// `kind` values.
pub const JOURNAL_KINDS: &[&str] = &[
    "conversation",
    "progress",
    "compact_summary",
    "outcome",
    "note",
    "handover",
    // A tracker item's status moved (work graph M3), on the conversation of
    // each live session working on it.
    "status_change",
    // The item moved out of `done` (work graph M7): on the live sessions'
    // conversations, else the newest past session's last one.
    "reopened",
    // Tidy-up acted on the session (work graph M7): archive, kill, safe kill.
    "tidy",
    // A Claude-written summary of a dead session's conversation, on demand
    // (work graph M13.4c, D10): one per conversation, from `agent`.
    "summary",
    // Fleet wrote to the item's tracker (work graph M13.4e, D3): the PR as
    // a remote link, on the session's conversation.
    "write_back",
    // An agent's own todo/task step (design 2026-09-29 §2); meta {native_id, state, agent}.
    "step",
];

/// `source` values.
pub const JOURNAL_SOURCES: &[&str] = &["hook", "transcript", "probe", "agent", "fleet"];

/// Newest `progress` rows kept per conversation.
pub const PROGRESS_CAP: usize = 5;
/// Newest `compact_summary` rows kept per conversation.
pub const COMPACT_SUMMARY_CAP: usize = 3;
/// Longest stored compaction summary (chars).
pub const COMPACT_SUMMARY_MAX_CHARS: usize = 12_000;
/// Longest stored body of any other row (chars).
pub const JOURNAL_BODY_MAX_CHARS: usize = 12_000;

/// One journal row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalRow {
    pub id: i64,
    #[serde(default)]
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub participant_id: Option<i64>,
    pub at: i64,
    pub kind: String,
    pub source: String,
    #[serde(default)]
    pub body: Option<String>,
    /// JSON object (the `conversation` row: turns, compactions, span, host,
    /// tmux, name, branch, end_reason …).
    #[serde(default)]
    pub meta: Option<String>,
    #[serde(default)]
    pub delivered_at: Option<i64>,
}

const COLUMNS: &str =
    "id, claude_session_id, participant_id, at, kind, source, body, meta, delivered_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        id: r.get(0)?,
        claude_session_id: r.get(1)?,
        participant_id: r.get(2)?,
        at: r.get(3)?,
        kind: r.get(4)?,
        source: r.get(5)?,
        body: r.get(6)?,
        meta: r.get(7)?,
        delivered_at: r.get(8)?,
    })
}

fn cap_kind(kind: &str) -> Option<usize> {
    match kind {
        "progress" => Some(PROGRESS_CAP),
        "compact_summary" => Some(COMPACT_SUMMARY_CAP),
        "step" => Some(crate::service::work::steps::STEP_CAP),
        _ => None,
    }
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Conversation ids that belong to work `key` (already normalised): every id
/// an ENDED confirmed link snapshotted, plus every conversation of the
/// session behind a LIVE confirmed link. The retention sweep keeps these
/// while the work is not done (`store::work_retention`).
const KEY_CONVERSATIONS: &str = "\
    WITH links AS ( \
      SELECT l.* FROM work_links l LEFT JOIN work_items i ON i.id = l.item_id \
      WHERE l.state = 'confirmed' AND (l.ref_key = ?1 OR i.key = ?1)) \
    SELECT j.value FROM links l, json_each(l.snap_claude_ids) j \
      WHERE l.ended_at IS NOT NULL AND l.snap_claude_ids IS NOT NULL \
    UNION \
    SELECT c.claude_session_id FROM links l \
      JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
      JOIN conversations c ON c.session_id = p.session_id \
      WHERE l.ended_at IS NULL";

/// A link's journal window (task → session P-7). A Switch moves a session
/// from one task to another WITHOUT a new conversation, so "the link's
/// conversations" would hand the old task the new task's work and the other
/// way round. A link's rows are therefore the rows of its conversations
/// written from the moment the session last switched AWAY from something
/// before the link began (`lo`) until the link itself was switched away
/// (`hi`, exclusive). A link no switch touched has neither bound and reads
/// exactly as before: linking a session after it did the work still files
/// that work under the task. `{l}` is the link's alias in the caller's
/// query.
const WINDOW_LO: &str = "(SELECT MAX(x.ended_at) FROM work_links x \
      WHERE x.participant_id = {l}.participant_id AND x.end_reason = 'switched' \
        AND x.id != {l}.id AND x.ended_at <= {l}.created_at)";
const WINDOW_HI: &str = "CASE WHEN {l}.end_reason = 'switched' THEN {l}.ended_at END";

/// The window bounds of [`WINDOW_LO`] / [`WINDOW_HI`] for a link aliased `l`.
fn window_sql(alias: &str) -> (String, String) {
    (
        WINDOW_LO.replace("{l}", alias),
        WINDOW_HI.replace("{l}", alias),
    )
}

impl Store {
    /// Append one journal row and enforce its conversation's cap in the same
    /// transaction. A `conversation` row is upserted (one per conversation).
    /// Returns `None` when the row was skipped: an empty body for a
    /// `progress` / `compact_summary` row, or an exact repeat of that
    /// conversation's newest row of the same kind (those two kinds only).
    pub fn append_journal(
        &self,
        claude_session_id: Option<&str>,
        participant_id: Option<i64>,
        kind: &str,
        source: &str,
        body: Option<&str>,
        meta: Option<&str>,
    ) -> Result<Option<i64>, IpcError> {
        if !JOURNAL_KINDS.contains(&kind) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown journal kind {kind:?}"),
            ));
        }
        if !JOURNAL_SOURCES.contains(&source) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown journal source {source:?}"),
            ));
        }
        if claude_session_id.is_none() && kind != "handover" {
            return Err(IpcError::new(
                codes::E_INVALID,
                "only a handover row may lack a claude_session_id",
            ));
        }
        let max = if kind == "compact_summary" {
            COMPACT_SUMMARY_MAX_CHARS
        } else {
            JOURNAL_BODY_MAX_CHARS
        };
        let body = body.map(str::trim).filter(|b| !b.is_empty());
        let body = body.map(|b| cap_chars(b, max));
        if body.is_none() && matches!(kind, "progress" | "compact_summary" | "handover") {
            return Ok(None);
        }
        let now = now_unix();
        // SAVEPOINT (`Store::in_savepoint`), not a second `BEGIN`:
        // `Store::atomically` cannot nest, and Task 3 calls this (via
        // `journal_for_session`, from the Stop hook) from inside one. A
        // SAVEPOINT works both standalone (autocommit — it starts an
        // implicit transaction) and already inside an open transaction.
        self.in_savepoint("append_journal", |_| -> Result<Option<i64>, IpcError> {
            // Only `progress` / `compact_summary` skip a repeated body: a
            // `step` row repeats its text when only its state moves, and
            // `Store::record_steps` already drops events that change nothing.
            if matches!(kind, "progress" | "compact_summary") {
                let last: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT body FROM work_journal WHERE claude_session_id = ?1 AND kind = ?2 \
                         ORDER BY at DESC, id DESC LIMIT 1",
                        rusqlite::params![claude_session_id, kind],
                        |r| r.get(0),
                    )
                    .ok()
                    .flatten();
                // Best-effort dedupe read, unless SQLite answered it by
                // rolling the savepoint back: the INSERT below would then
                // commit on its own.
                self.ensure_in_savepoint()?;
                if last.is_some() && last == body {
                    return Ok(None);
                }
            }
            let id = if kind == "conversation" {
                self.conn.query_row(
                    "INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, \
                                               body, meta) \
                     VALUES (?1, ?2, ?3, 'conversation', ?4, ?5, ?6) \
                     ON CONFLICT(claude_session_id) WHERE kind = 'conversation' DO UPDATE SET \
                       at = excluded.at, \
                       body = COALESCE(excluded.body, work_journal.body), \
                       meta = COALESCE(excluded.meta, work_journal.meta), \
                       participant_id = COALESCE(work_journal.participant_id, excluded.participant_id) \
                     RETURNING id",
                    rusqlite::params![claude_session_id, participant_id, now, source, body, meta],
                    |r| r.get(0),
                )?
            } else {
                self.conn.execute(
                    "INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, \
                                               body, meta) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![claude_session_id, participant_id, now, kind, source, body, meta],
                )?;
                self.conn.last_insert_rowid()
            };
            if let Some(cap) = cap_kind(kind) {
                self.conn.execute(
                    "DELETE FROM work_journal WHERE claude_session_id = ?1 AND kind = ?2 \
                       AND id NOT IN (SELECT id FROM work_journal \
                                      WHERE claude_session_id = ?1 AND kind = ?2 \
                                      ORDER BY at DESC, id DESC LIMIT ?3)",
                    rusqlite::params![claude_session_id, kind, cap as i64],
                )?;
            }
            Ok(Some(id))
        })
    }

    /// Store `claude_session_id`'s summary (work graph M13.4c): a `summary`
    /// row from `agent`, replacing any earlier one of that conversation, so
    /// there is at most one per conversation. `body` is already redacted and
    /// capped by the caller; `append_journal` caps it again. Returns the new
    /// row's id.
    pub fn replace_summary(
        &self,
        claude_session_id: &str,
        body: &str,
        meta: Option<&str>,
    ) -> Result<i64, IpcError> {
        self.in_savepoint("replace_summary", |conn| -> Result<i64, IpcError> {
            conn.execute(
                "DELETE FROM work_journal WHERE claude_session_id = ?1 AND kind = 'summary'",
                [claude_session_id],
            )?;
            self.append_journal(
                Some(claude_session_id),
                None,
                "summary",
                "agent",
                Some(body),
                meta,
            )?
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "an empty summary is not stored"))
        })
    }

    /// Journal a harvested row for the session `session_id` is running
    /// (`progress` from a Stop, `compact_summary` from a compaction). The
    /// row's participant is recorded when it has one. Best-effort callers
    /// ignore the error.
    pub fn journal_for_session(
        &self,
        session_id: i64,
        claude_session_id: &str,
        kind: &str,
        source: &str,
        body: &str,
    ) -> Result<Option<i64>, IpcError> {
        let participant = self.participant_for_session(session_id)?.map(|p| p.id);
        self.append_journal(
            Some(claude_session_id),
            participant,
            kind,
            source,
            Some(body),
            None,
        )
    }

    /// Every row of these conversations except handovers, oldest first.
    pub fn journal_for_conversations(&self, ids: &[String]) -> Result<Vec<JournalRow>, IpcError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let json = serde_json::to_string(ids)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM work_journal \
             WHERE kind != 'handover' \
               AND claude_session_id IN (SELECT value FROM json_each(?1)) \
             ORDER BY at ASC, id ASC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![json], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The conversation ids that belong to work `key`: those of its ended
    /// links' snapshots and of its live links' sessions.
    pub fn work_conversation_ids(&self, key: &str) -> Result<Vec<String>, IpcError> {
        let key = super::normalize_work_ref(key)?;
        let mut stmt = self.conn.prepare(KEY_CONVERSATIONS)?;
        let rows = stmt.query_map(rusqlite::params![key], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The journal of work `key`, through both live and ended links, oldest
    /// first, each link's rows inside its window (P-7, [`WINDOW_LO`]).
    /// Handover rows are never part of it.
    pub fn journal_for_key(&self, key: &str) -> Result<Vec<JournalRow>, IpcError> {
        let key = super::normalize_work_ref(key)?;
        let (lo, hi) = window_sql("l");
        let mut stmt = self.conn.prepare(&format!(
            "WITH links AS ( \
               SELECT l.*, {lo} AS lo, {hi} AS hi FROM work_links l \
               LEFT JOIN work_items i ON i.id = l.item_id \
               WHERE l.state = 'confirmed' AND (l.ref_key = ?1 OR i.key = ?1)), \
             wins(cid, lo, hi) AS ( \
               SELECT j.value, l.lo, l.hi FROM links l, json_each(l.snap_claude_ids) j \
                 WHERE l.ended_at IS NOT NULL AND l.snap_claude_ids IS NOT NULL \
               UNION ALL \
               SELECT c.claude_session_id, l.lo, l.hi FROM links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 JOIN conversations c ON c.session_id = p.session_id \
                 WHERE l.ended_at IS NULL) \
             SELECT {COLUMNS} FROM work_journal w \
             WHERE w.kind != 'handover' AND EXISTS ( \
               SELECT 1 FROM wins WHERE wins.cid = w.claude_session_id \
                 AND (wins.lo IS NULL OR w.at >= wins.lo) \
                 AND (wins.hi IS NULL OR w.at < wins.hi)) \
             ORDER BY w.at ASC, w.id ASC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![key], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The journal window of link `link_id` (P-7): `(from, until)`, each
    /// `None` when unbounded. `(None, None)` for a link no switch touched,
    /// and for one that is gone.
    pub fn link_journal_window(
        &self,
        link_id: i64,
    ) -> Result<(Option<i64>, Option<i64>), IpcError> {
        let (lo, hi) = window_sql("l");
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {lo}, {hi} FROM work_links l WHERE l.id = ?1"),
                rusqlite::params![link_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, None)))
    }

    /// [`Self::journal_for_conversations`] inside a link's window
    /// ([`Self::link_journal_window`]).
    pub fn journal_for_conversations_within(
        &self,
        ids: &[String],
        (from, until): (Option<i64>, Option<i64>),
    ) -> Result<Vec<JournalRow>, IpcError> {
        let mut rows = self.journal_for_conversations(ids)?;
        rows.retain(|r| from.is_none_or(|f| r.at >= f) && until.is_none_or(|u| r.at < u));
        Ok(rows)
    }

    /// Queue a handover brief for `session_id`'s next hook delivery.
    pub fn enqueue_handover(
        &self,
        session_id: i64,
        body: &str,
        meta: Option<&str>,
    ) -> Result<i64, IpcError> {
        let participant = self.work_participant_of(session_id)?;
        self.append_journal(
            None,
            Some(participant),
            "handover",
            "fleet",
            Some(body),
            meta,
        )?
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "a handover needs a body"))
    }

    /// Handover rows addressed to `session_id` that no hook has carried yet,
    /// oldest first.
    pub fn undelivered_handovers(&self, session_id: i64) -> Result<Vec<JournalRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM work_journal \
             WHERE kind = 'handover' AND delivered_at IS NULL AND participant_id = \
               (SELECT id FROM participants WHERE session_id = ?1 AND retired_at IS NULL) \
             ORDER BY id ASC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![session_id], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Stamp handover rows delivered, and bind them to the conversation that
    /// received them.
    pub fn mark_handovers_delivered(
        &self,
        ids: &[i64],
        claude_session_id: Option<&str>,
    ) -> Result<(), IpcError> {
        let now = now_unix();
        for id in ids {
            self.conn.execute(
                "UPDATE work_journal SET delivered_at = ?1, \
                   claude_session_id = COALESCE(claude_session_id, ?2) \
                 WHERE id = ?3 AND kind = 'handover' AND delivered_at IS NULL",
                rusqlite::params![now, claude_session_id, id],
            )?;
        }
        Ok(())
    }

    /// The live participant of an existing session (minted if missing).
    fn work_participant_of(&self, session_id: i64) -> Result<i64, IpcError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
            ));
        }
        self.ensure_participant_for_session(session_id)
    }
}

/// One step's newest state in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StepView {
    pub claude_session_id: String,
    pub native_id: String,
    pub text: String,
    pub state: String,
    pub at: i64,
}

impl Store {
    /// Append the events that change a step's known text or state; each
    /// becomes one `step` row (body = text, meta = {native_id, state, agent}).
    /// Returns how many rows it wrote.
    pub fn record_steps(
        &self,
        claude_session_id: &str,
        participant_id: Option<i64>,
        source: &str,
        events: &[crate::service::work::steps::StepEvent],
    ) -> Result<usize, IpcError> {
        let mut known: std::collections::HashMap<String, StepView> = self
            .current_steps(&[claude_session_id.to_string()])?
            .into_iter()
            .map(|v| (v.native_id.clone(), v))
            .collect();
        let mut wrote = 0;
        for e in events {
            let prev = known.get(&e.native_id);
            let text = e.text.clone().or_else(|| prev.map(|p| p.text.clone()));
            let Some(text) = text else { continue };
            let state = e
                .state
                .map(|s| s.as_str().to_string())
                .or_else(|| prev.map(|p| p.state.clone()))
                .unwrap_or_else(|| "pending".into());
            if prev.is_some_and(|p| p.text == text && p.state == state) {
                continue;
            }
            let meta = serde_json::json!({
                "native_id": e.native_id,
                "state": state,
                "agent": e.agent,
            })
            .to_string();
            if self
                .append_journal(
                    Some(claude_session_id),
                    participant_id,
                    "step",
                    source,
                    Some(&text),
                    Some(&meta),
                )?
                .is_some()
            {
                wrote += 1;
                // A batch (one TodoWrite snapshot, a transcript tail) can
                // name the same step twice; later events compare to this one.
                known.insert(
                    e.native_id.clone(),
                    StepView {
                        claude_session_id: claude_session_id.to_string(),
                        native_id: e.native_id.clone(),
                        text,
                        state,
                        at: now_unix(),
                    },
                );
            }
        }
        Ok(wrote)
    }

    /// The newest state of every step in these conversations, in order of
    /// first appearance.
    pub fn current_steps(&self, conversation_ids: &[String]) -> Result<Vec<StepView>, IpcError> {
        if conversation_ids.is_empty() {
            return Ok(Vec::new());
        }
        let json = serde_json::to_string(conversation_ids)
            .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        let mut stmt = self.conn.prepare(
            "SELECT claude_session_id, json_extract(meta, '$.native_id') AS nid, body, \
                    json_extract(meta, '$.state'), at, id \
               FROM work_journal \
              WHERE kind = 'step' AND claude_session_id IN (SELECT value FROM json_each(?1)) \
              ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![json], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut order: Vec<(String, String)> = Vec::new();
        let mut last: std::collections::HashMap<(String, String), StepView> =
            std::collections::HashMap::new();
        for row in rows {
            let (conv, nid, body, state, at) = row?;
            let k = (conv.clone(), nid.clone());
            if !last.contains_key(&k) {
                order.push(k.clone());
            }
            last.insert(
                k,
                StepView {
                    claude_session_id: conv,
                    native_id: nid,
                    text: body.unwrap_or_default(),
                    state,
                    at,
                },
            );
        }
        Ok(order.into_iter().filter_map(|k| last.remove(&k)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{StartSource, WorkTarget};

    fn seed(s: &Store, name: &str, claude: &str) -> i64 {
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.rebind_conversation(id, claude, StartSource::Startup, None, None)
            .unwrap();
        id
    }

    fn kinds(rows: &[JournalRow]) -> Vec<(&str, Option<&str>)> {
        rows.iter()
            .map(|r| (r.kind.as_str(), r.body.as_deref()))
            .collect()
    }

    #[test]
    fn progress_and_summaries_are_capped_per_conversation_and_repeats_skipped() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        for i in 0..8 {
            s.journal_for_session(sid, "c1", "progress", "hook", &format!("turn {i}"))
                .unwrap();
        }
        assert_eq!(
            s.journal_for_session(sid, "c1", "progress", "hook", "turn 7")
                .unwrap(),
            None,
            "an exact repeat is skipped"
        );
        assert_eq!(
            s.journal_for_session(sid, "c1", "progress", "hook", "  ")
                .unwrap(),
            None,
            "an empty detail is skipped"
        );
        for i in 0..5 {
            s.journal_for_session(sid, "c1", "compact_summary", "transcript", &format!("s{i}"))
                .unwrap();
        }
        let rows = s.journal_for_conversations(&["c1".into()]).unwrap();
        let progress: Vec<_> = rows.iter().filter(|r| r.kind == "progress").collect();
        assert_eq!(progress.len(), PROGRESS_CAP);
        assert_eq!(progress.last().unwrap().body.as_deref(), Some("turn 7"));
        assert_eq!(progress[0].body.as_deref(), Some("turn 3"));
        let sums: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == "compact_summary")
            .collect();
        assert_eq!(sums.len(), COMPACT_SUMMARY_CAP);
        assert_eq!(sums[0].body.as_deref(), Some("s2"));
        // A long summary is capped.
        let long = "x".repeat(COMPACT_SUMMARY_MAX_CHARS + 50);
        let id = s
            .journal_for_session(sid, "c1", "compact_summary", "transcript", &long)
            .unwrap()
            .unwrap();
        let body: String = s
            .conn
            .query_row("SELECT body FROM work_journal WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body.chars().count(), COMPACT_SUMMARY_MAX_CHARS);
    }

    #[test]
    fn a_closed_conversation_gets_one_row_that_survives_the_session() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        s.conversation_set_first_prompt(sid, "c1", "fix the login bug")
            .unwrap();
        s.conversation_bump_turns(sid, "c1").unwrap();
        s.journal_for_session(sid, "c1", "progress", "hook", "done step 1")
            .unwrap();
        s.close_conversation(sid, "c1", "clear").unwrap();
        s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
            .unwrap();
        s.delete_session(sid).unwrap();

        let rows = s
            .journal_for_conversations(&["c1".into(), "c2".into()])
            .unwrap();
        let convs: Vec<_> = rows.iter().filter(|r| r.kind == "conversation").collect();
        assert_eq!(
            convs.len(),
            2,
            "the closed one and the one the delete ended"
        );
        let c1 = convs
            .iter()
            .find(|r| r.claude_session_id.as_deref() == Some("c1"))
            .unwrap();
        assert_eq!(c1.body.as_deref(), Some("fix the login bug"));
        let meta: serde_json::Value = serde_json::from_str(c1.meta.as_deref().unwrap()).unwrap();
        assert_eq!(meta["turns"], 1);
        assert_eq!(meta["end_reason"], "clear");
        assert_eq!(meta["host"], "h");
        assert_eq!(meta["tmux"], "dev");
        let c2 = convs
            .iter()
            .find(|r| r.claude_session_id.as_deref() == Some("c2"))
            .unwrap();
        let meta: serde_json::Value = serde_json::from_str(c2.meta.as_deref().unwrap()).unwrap();
        assert_eq!(meta["end_reason"], "session_deleted");
        assert!(rows.iter().any(|r| r.kind == "progress"));
    }

    #[test]
    fn the_journal_survives_a_move_and_a_participant_sweep() {
        let s = Store::open_in_memory().unwrap();
        let src = seed(&s, "src", "c1");
        let dst = seed(&s, "dst", "c9");
        s.journal_for_session(src, "c1", "progress", "hook", "before the move")
            .unwrap();
        let p = s.participant_for_session(src).unwrap().unwrap().id;
        s.repoint_participant(p, dst).unwrap();
        s.delete_session(src).unwrap();
        s.delete_session(dst).unwrap();
        // Sweep every retired participant.
        s.sweep_retired_participants(now_unix() + 10 * 365 * 86_400, 0)
            .unwrap();
        let rows = s.journal_for_conversations(&["c1".into()]).unwrap();
        assert_eq!(
            kinds(&rows),
            vec![
                ("progress", Some("before the move")),
                ("conversation", None)
            ]
        );
        assert!(rows.iter().all(|r| r.participant_id.is_none()));
    }

    #[test]
    fn a_key_finds_its_journal_through_live_and_ended_links() {
        let s = Store::open_in_memory().unwrap();
        let old = seed(&s, "old", "c-old");
        let live = seed(&s, "live", "c-live");
        let other = seed(&s, "other", "c-other");
        s.link_session_work(old, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.link_session_work(live, WorkTarget::Key("abc-1"), "manual")
            .unwrap();
        s.link_session_work(other, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        for (sid, c) in [(old, "c-old"), (live, "c-live"), (other, "c-other")] {
            s.journal_for_session(sid, c, "progress", "hook", &format!("p {c}"))
                .unwrap();
        }
        s.delete_session(old).unwrap();
        let mut ids = s.work_conversation_ids("abc-1").unwrap();
        ids.sort();
        assert_eq!(ids, vec!["c-live".to_string(), "c-old".to_string()]);
        let rows = s.journal_for_key("ABC-1").unwrap();
        let bodies: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == "progress")
            .map(|r| r.body.clone().unwrap())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert!(!bodies.contains(&"p c-other".to_string()));
    }

    #[test]
    fn a_handover_is_delivered_once_and_never_part_of_the_journal() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        let id = s.enqueue_handover(sid, "brief", None).unwrap();
        assert_eq!(s.undelivered_handovers(sid).unwrap().len(), 1);
        s.mark_handovers_delivered(&[id], Some("c1")).unwrap();
        assert!(s.undelivered_handovers(sid).unwrap().is_empty());
        assert!(s
            .journal_for_conversations(&["c1".into()])
            .unwrap()
            .is_empty());
        assert_eq!(
            s.enqueue_handover(999, "x", None).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn bad_kinds_sources_and_missing_ids_are_refused() {
        let s = Store::open_in_memory().unwrap();
        for (cid, kind, source) in [
            (Some("c"), "gossip", "hook"),
            (Some("c"), "note", "rumour"),
            (None, "note", "agent"),
        ] {
            assert_eq!(
                s.append_journal(cid, None, kind, source, Some("x"), None)
                    .unwrap_err()
                    .code,
                codes::E_INVALID
            );
        }
    }

    #[test]
    fn steps_record_only_changes_and_read_back_their_newest_state() {
        use crate::service::work::steps::{StepEvent, StepState};
        let s = Store::open_in_memory().unwrap();
        let ev = |id: &str, text: Option<&str>, st: StepState| StepEvent {
            native_id: id.into(),
            text: text.map(str::to_string),
            state: Some(st),
            agent: "claude_code",
        };
        assert_eq!(
            s.record_steps(
                "c1",
                None,
                "hook",
                &[ev("task:1", Some("Read OM-110"), StepState::Pending)]
            )
            .unwrap(),
            1
        );
        assert_eq!(
            s.record_steps(
                "c1",
                None,
                "hook",
                &[ev("task:1", Some("Read OM-110"), StepState::Pending)]
            )
            .unwrap(),
            0,
            "no change, no row"
        );
        assert_eq!(
            s.record_steps(
                "c1",
                None,
                "hook",
                &[ev("task:1", None, StepState::Completed)]
            )
            .unwrap(),
            1
        );
        let cur = s.current_steps(&["c1".to_string()]).unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(
            (cur[0].text.as_str(), cur[0].state.as_str()),
            ("Read OM-110", "completed")
        );
    }

    #[test]
    fn steps_are_capped_per_conversation() {
        use crate::service::work::steps::{StepEvent, StepState, STEP_CAP};
        let s = Store::open_in_memory().unwrap();
        for i in 0..(STEP_CAP + 5) {
            s.record_steps(
                "c1",
                None,
                "hook",
                &[StepEvent {
                    native_id: format!("todo:{i}"),
                    text: Some(format!("s{i}")),
                    state: Some(StepState::Pending),
                    agent: "claude_code",
                }],
            )
            .unwrap();
        }
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM work_journal WHERE kind = 'step'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n as usize, STEP_CAP);
    }

    // ── task → session P-2 / P-7: switch and link windows ──────────────

    /// Put the journal row with `body` at `at`.
    fn at(s: &Store, body: &str, at: i64) {
        s.conn
            .execute(
                "UPDATE work_journal SET at = ?2 WHERE body = ?1",
                rusqlite::params![body, at],
            )
            .unwrap();
    }

    fn bodies(rows: &[JournalRow]) -> Vec<String> {
        rows.iter()
            .filter(|r| r.kind == "progress")
            .filter_map(|r| r.body.clone())
            .collect()
    }

    #[test]
    fn a_switch_ends_the_old_link_with_its_snapshot_and_moves_the_primary() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        let a = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let b = s
            .switch_session_work(sid, a.id, WorkTarget::Key("DEF-2"), "manual", Some(a.id))
            .unwrap();
        let old = s.get_work_link(a.id).unwrap().unwrap();
        assert!(
            old.ended_at.is_some(),
            "the old link ends, it is not removed"
        );
        assert_eq!(old.end_reason.as_deref(), Some("switched"));
        assert!(!old.is_primary);
        assert_eq!(old.snap_claude_ids.as_deref(), Some(r#"["c1"]"#));
        assert!(b.is_primary);
        assert_eq!(b.ref_key.as_deref(), Some("DEF-2"));
        assert_eq!(s.current_primary_link(sid).unwrap(), Some(b.id));
        let live: Vec<i64> = s
            .session_work_links(sid)
            .unwrap()
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(live, vec![b.id], "one live link: the new one");
    }

    #[test]
    fn a_stale_or_empty_switch_changes_nothing() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        let a = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let stale = s
            .switch_session_work(
                sid,
                a.id,
                WorkTarget::Key("DEF-2"),
                "manual",
                Some(a.id + 9),
            )
            .unwrap_err();
        assert_eq!(stale.code, codes::E_CONFLICT);
        let same = s
            .switch_session_work(sid, a.id, WorkTarget::Key("abc-1"), "manual", None)
            .unwrap_err();
        assert_eq!(same.code, codes::E_INVALID);
        let gone = s
            .switch_session_work(sid, a.id + 50, WorkTarget::Key("DEF-2"), "manual", None)
            .unwrap_err();
        assert_eq!(gone.code, codes::E_NOTFOUND);
        let links = s.session_work_links(sid).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].id, a.id);
        assert!(links[0].ended_at.is_none() && links[0].is_primary);
    }

    #[test]
    fn a_switch_without_clear_splits_the_conversation_between_the_two_tasks() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        let a = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.journal_for_session(sid, "c1", "progress", "hook", "on A")
            .unwrap();
        let now = now_unix();
        at(&s, "on A", now - 100);
        let b = s
            .switch_session_work(sid, a.id, WorkTarget::Key("DEF-2"), "manual", None)
            .unwrap();
        s.journal_for_session(sid, "c1", "progress", "hook", "on B")
            .unwrap();
        at(&s, "on B", now + 100);
        assert_eq!(bodies(&s.journal_for_key("ABC-1").unwrap()), vec!["on A"]);
        assert_eq!(bodies(&s.journal_for_key("DEF-2").unwrap()), vec!["on B"]);
        // The task page's last outcome reads the ended link inside its window.
        let window = s.link_journal_window(a.id).unwrap();
        assert_eq!(window.0, None);
        assert!(window.1.is_some());
        let rows = s
            .journal_for_conversations_within(&["c1".into()], window)
            .unwrap();
        assert_eq!(bodies(&rows), vec!["on A"]);
        assert_eq!(s.link_journal_window(b.id).unwrap().1, None);
        assert!(s.link_journal_window(b.id).unwrap().0.is_some());
    }

    #[test]
    fn a_switch_back_after_clear_keeps_each_task_its_own_conversation() {
        let s = Store::open_in_memory().unwrap();
        let sid = seed(&s, "dev", "c1");
        let a = s
            .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.journal_for_session(sid, "c1", "progress", "hook", "A in c1")
            .unwrap();
        let now = now_unix();
        at(&s, "A in c1", now - 300);
        let b = s
            .switch_session_work(sid, a.id, WorkTarget::Key("DEF-2"), "manual", None)
            .unwrap();
        // `/clear`: a fresh conversation on task B.
        s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
            .unwrap();
        s.journal_for_session(sid, "c2", "progress", "hook", "B in c2")
            .unwrap();
        at(&s, "B in c2", now + 100);
        // Back to A later on: A's new link starts at that switch.
        let back = s
            .switch_session_work(sid, b.id, WorkTarget::Key("ABC-1"), "manual", Some(b.id))
            .unwrap();
        s.conn
            .execute(
                "UPDATE work_links SET ended_at = ?2 WHERE id = ?1",
                rusqlite::params![b.id, now + 200],
            )
            .unwrap();
        s.conn
            .execute(
                "UPDATE work_links SET created_at = ?2 WHERE id = ?1",
                rusqlite::params![back.id, now + 200],
            )
            .unwrap();
        s.journal_for_session(sid, "c2", "progress", "hook", "A again")
            .unwrap();
        at(&s, "A again", now + 300);
        assert_eq!(
            bodies(&s.journal_for_key("ABC-1").unwrap()),
            vec!["A in c1", "A again"]
        );
        assert_eq!(
            bodies(&s.journal_for_key("DEF-2").unwrap()),
            vec!["B in c2"]
        );
        // A's first snapshot is its own conversation: Continue resumes c1.
        let first = s.get_work_link(a.id).unwrap().unwrap();
        assert_eq!(first.snap_claude_ids.as_deref(), Some(r#"["c1"]"#));
        // c1 ran on into B until the `/clear`, so B keeps both; its journal
        // window keeps A's c1 rows out, and Continue on B resumes c2.
        let b_old = s.get_work_link(b.id).unwrap().unwrap();
        assert_eq!(b_old.snap_claude_ids.as_deref(), Some(r#"["c1","c2"]"#));
    }

    #[test]
    fn live_sessions_on_a_target_name_every_other_live_session() {
        let s = Store::open_in_memory().unwrap();
        let one = seed(&s, "one", "c1");
        let two = seed(&s, "two", "c2");
        let three = seed(&s, "three", "c3");
        for sid in [one, two] {
            s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
                .unwrap();
        }
        s.link_session_work(three, WorkTarget::Key("DEF-2"), "manual")
            .unwrap();
        assert_eq!(
            s.live_sessions_on_target(WorkTarget::Key("abc-1"), one)
                .unwrap(),
            vec![two]
        );
        assert!(s
            .live_sessions_on_target(WorkTarget::Key("XYZ-9"), one)
            .unwrap()
            .is_empty());
    }
}
