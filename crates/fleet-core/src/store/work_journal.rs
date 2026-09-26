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

impl Store {
    /// Append one journal row and enforce its conversation's cap in the same
    /// transaction. A `conversation` row is upserted (one per conversation).
    /// Returns `None` when the row was skipped: an empty body for a
    /// `progress` / `compact_summary` row, or an exact repeat of that
    /// conversation's newest row of the same kind.
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
            if cap_kind(kind).is_some() {
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
    /// first. Handover rows are never part of it.
    pub fn journal_for_key(&self, key: &str) -> Result<Vec<JournalRow>, IpcError> {
        let ids = self.work_conversation_ids(key)?;
        self.journal_for_conversations(&ids)
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
}
