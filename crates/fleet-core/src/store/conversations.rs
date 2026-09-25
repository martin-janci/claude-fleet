//! Per-session conversation tracking (migration 037). `rebind_conversation`
//! is the ONLY writer of `sessions.claude_session_id` outside the reconcile
//! upsert, which then opens the conversation through it (the fallback rebind,
//! spec §1.4); see the spec's §1.3.

use super::*;
use crate::ipc_error::IpcError;

/// `awaiting_rebind_at` older than this is ignored (spec §1.3).
pub const AWAITING_REBIND_TTL_SECS: i64 = 300;
/// A second compaction signal within this window is the same compaction
/// (SessionStart(compact) and PostCompact both fire).
const COMPACT_DEDUPE_SECS: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartSource {
    Startup,
    Resume,
    Clear,
    Compact,
    Fork,
    Fleet,
    Unknown,
}

impl StartSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StartSource::Startup => "startup",
            StartSource::Resume => "resume",
            StartSource::Clear => "clear",
            StartSource::Compact => "compact",
            StartSource::Fork => "fork",
            StartSource::Fleet => "fleet",
            StartSource::Unknown => "unknown",
        }
    }
    /// Map a SessionStart `source`. Anything unrecognised is `Unknown`.
    pub fn from_hook(s: &str) -> Self {
        match s {
            "startup" => StartSource::Startup,
            "resume" => StartSource::Resume,
            "clear" => StartSource::Clear,
            "compact" => StartSource::Compact,
            "fork" => StartSource::Fork,
            _ => StartSource::Unknown,
        }
    }
    /// A brand-new, empty conversation: context is 0 and per-turn fields reset.
    pub fn resets_context(self) -> bool {
        matches!(
            self,
            StartSource::Startup | StartSource::Clear | StartSource::Fleet
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConversationRow {
    pub id: i64,
    pub session_id: i64,
    pub claude_session_id: String,
    // The `Option` fields carry `serde(default)` because a desktop in remote
    // mode deserialises this row straight out of a hub's tool result, and a
    // hub that answers with `ok_json_compact` strips every null key (see
    // `src-tauri/src/backend/contract.rs`). Without them a conversation that
    // has, say, no `end_reason` would fail to parse instead of arriving as
    // `None`.
    #[serde(default)]
    pub transcript_path: Option<String>,
    pub started_at: i64,
    #[serde(default)]
    pub ended_at: Option<i64>,
    pub start_source: String,
    #[serde(default)]
    pub end_reason: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub first_prompt: Option<String>,
    pub turns: i64,
    pub compactions: i64,
    /// True for the session's current conversation.
    pub current: bool,
}

/// Columns of [`ConversationRow`], in [`map_conversation`]'s order.
const CONVERSATION_SELECT: &str =
    "SELECT c.id, c.session_id, c.claude_session_id, c.transcript_path, c.started_at, \
            c.ended_at, c.start_source, c.end_reason, c.model, c.first_prompt, c.turns, \
            c.compactions, \
            (c.claude_session_id IS s.claude_session_id AND c.ended_at IS NULL) \
     FROM conversations c JOIN sessions s ON s.id = c.session_id";

fn map_conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        claude_session_id: r.get(2)?,
        transcript_path: r.get(3)?,
        started_at: r.get(4)?,
        ended_at: r.get(5)?,
        start_source: r.get(6)?,
        end_reason: r.get(7)?,
        model: r.get(8)?,
        first_prompt: r.get(9)?,
        turns: r.get(10)?,
        compactions: r.get(11)?,
        current: r.get::<_, i64>(12)? != 0,
    })
}

impl Store {
    /// Make `claude_session_id` the session's current conversation.
    ///
    /// - Same id as the current one: ensure its conversation row is open,
    ///   apply the source's resets, no rebind.
    /// - Known id (a `/resume` back): reopen that row, no duplicate.
    /// - New id: insert a row.
    ///
    /// Every other open row of the session is closed with its pending
    /// `end_reason` (set by `close_conversation`) or `replaced`. The session
    /// row gets the new id, a transcript path that belongs to it (else NULL),
    /// the model, and `awaiting_rebind_at = NULL`; resetting sources zero the
    /// context and clear `current_activity` / `last_prompt` / `pending_input`
    /// (a dialog on the old conversation's pane is not one on the new
    /// conversation's), resume marks the context stale. A same-id call
    /// upgrades a `start_source` of `unknown` to `source` (a SessionStart
    /// arriving after the UserPromptSubmit that opened the conversation).
    /// One transaction; emits `session:updated` and `session:conversations`
    /// after commit.
    ///
    /// Opens its own transaction, so it cannot run inside
    /// `Store::atomically` (SQLite has no nested `BEGIN`).
    pub fn rebind_conversation(
        &self,
        session_id: i64,
        claude_session_id: &str,
        source: StartSource,
        transcript_path: Option<&str>,
        model: Option<&str>,
    ) -> Result<Option<SessionRow>, IpcError> {
        self.rebind_conversation_opts(
            session_id,
            claude_session_id,
            source,
            transcript_path,
            model,
            false,
        )
    }

    /// [`Self::rebind_conversation`] for a conversation whose first turn has
    /// already begun (`turn_started`): the prompt that started it is kept
    /// (`last_prompt`, `current_activity` untouched), and a same-id call —
    /// a SessionStart that lost the race to its UserPromptSubmit — resets
    /// nothing, since the context already belongs to this conversation. A
    /// new id with a resetting source still zeroes the context.
    pub fn rebind_conversation_opts(
        &self,
        session_id: i64,
        claude_session_id: &str,
        source: StartSource,
        transcript_path: Option<&str>,
        model: Option<&str>,
        turn_started: bool,
    ) -> Result<Option<SessionRow>, IpcError> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        let prior_id: Option<String> = tx
            .query_row(
                "SELECT claude_session_id FROM sessions WHERE id=?1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let same = prior_id.as_deref() == Some(claude_session_id);
        tx.execute(
            "UPDATE conversations SET ended_at = COALESCE(ended_at, ?3), \
                 end_reason = COALESCE(end_reason, 'replaced') \
             WHERE session_id = ?1 AND claude_session_id != ?2 AND ended_at IS NULL",
            rusqlite::params![session_id, claude_session_id, now],
        )?;
        tx.execute(
            "INSERT INTO conversations (session_id, claude_session_id, transcript_path, \
                 started_at, start_source, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(session_id, claude_session_id) DO UPDATE SET \
                 ended_at = NULL, end_reason = NULL, \
                 transcript_path = COALESCE(excluded.transcript_path, transcript_path), \
                 model = COALESCE(excluded.model, model)",
            rusqlite::params![
                session_id,
                claude_session_id,
                transcript_path,
                now,
                source.as_str(),
                model
            ],
        )?;
        if same && !matches!(source, StartSource::Unknown | StartSource::Compact) {
            tx.execute(
                "UPDATE conversations SET start_source = ?3 \
                 WHERE session_id = ?1 AND claude_session_id = ?2 AND start_source = 'unknown'",
                rusqlite::params![session_id, claude_session_id, source.as_str()],
            )?;
        }
        let path_sql = if same {
            "transcript_path = COALESCE(?3, transcript_path)"
        } else {
            "transcript_path = ?3"
        };
        let resets = source.resets_context() && !(same && turn_started);
        let reset_sql = if resets && turn_started {
            ", context_tokens = 0, context_pct = 0, context_source = 'hook', \
               context_at = ?5, context_stale = 0"
        } else if resets {
            ", context_tokens = 0, context_pct = 0, context_source = 'hook', \
               context_at = ?5, context_stale = 0, current_activity = NULL, last_prompt = NULL, \
               pending_input = NULL"
        } else if matches!(
            source,
            StartSource::Resume | StartSource::Unknown | StartSource::Fork
        ) && !same
        {
            ", context_stale = 1"
        } else {
            ""
        };
        let sql = format!(
            "UPDATE sessions SET claude_session_id = ?2, {path_sql}, \
                 model = COALESCE(?4, model), awaiting_rebind_at = NULL{reset_sql} \
             WHERE id = ?1"
        );
        // rusqlite rejects a bound parameter the statement does not use, and
        // only the resetting branch references `?5`.
        let params: [&dyn rusqlite::ToSql; 5] = [
            &session_id,
            &claude_session_id,
            &transcript_path,
            &model,
            &now,
        ];
        let n_params = if resets { 5 } else { 4 };
        tx.execute(&sql, &params[..n_params])?;
        // Work graph M2.2: a conversation that an ENDED work link snapshotted
        // is being resumed (from fleet's Resume, `claude --resume`, or a
        // `/resume`) — the work follows it. Best-effort: work links never
        // fail a rebind.
        if !same {
            if let Err(e) = self.carry_resumed_work(session_id, claude_session_id) {
                tracing::warn!(session_id, error = %e.message, "[work] resume carry failed");
            }
        }
        tx.commit()?;
        self.bus.conversations_changed(session_id);
        Ok(self.emit_session(session_id)?)
    }

    /// Record that the conversation ended (SessionEnd / kill). The session's
    /// `claude_session_id` is untouched: the next rebind replaces it.
    pub fn close_conversation(
        &self,
        session_id: i64,
        claude_session_id: &str,
        reason: &str,
    ) -> Result<(), IpcError> {
        let n = self.conn.execute(
            "UPDATE conversations SET ended_at = COALESCE(ended_at, ?3), end_reason = ?4 \
             WHERE session_id = ?1 AND claude_session_id = ?2",
            rusqlite::params![session_id, claude_session_id, now_unix(), reason],
        )?;
        if n > 0 {
            self.bus.conversations_changed(session_id);
        }
        Ok(())
    }

    /// Store a hook-validated transcript path on the session's conversation
    /// row for `claude_session_id`, so an earlier conversation can be read
    /// after the session has moved on. No event: `list_conversations` reads
    /// it on demand.
    pub fn set_conversation_transcript_path(
        &self,
        session_id: i64,
        claude_session_id: &str,
        path: &str,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE conversations SET transcript_path = ?3 \
             WHERE session_id = ?1 AND claude_session_id = ?2",
            rusqlite::params![session_id, claude_session_id, path],
        )?;
        Ok(())
    }

    /// Stamp `last_hook_at` for a hook write that has no status write of
    /// its own (a SessionStart rebind). The reconcile upsert keys its
    /// in-flight guard on it: a pass that probed before this instant keeps
    /// the row's status AND its `claude_session_id`, so it cannot write back
    /// the id the hook just replaced. No event: not a wire field.
    pub fn record_hook_seen(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET last_hook_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        Ok(())
    }

    /// SessionEnd(clear|resume): the next SessionStart / UserPromptSubmit
    /// from an unknown id in this row's cwd may rebind it (spec §1.2 step 3).
    /// Stamps `last_hook_at` as well (see [`Self::record_hook_seen`]).
    pub fn mark_awaiting_rebind(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET awaiting_rebind_at = ?2, last_hook_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        Ok(())
    }

    /// Whether the session's `awaiting_rebind_at` is within the TTL.
    pub fn is_awaiting_rebind(&self, session_id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1 AND awaiting_rebind_at >= ?2",
                rusqlite::params![session_id, now_unix() - AWAITING_REBIND_TTL_SECS],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Live rows on `host_alias` whose `awaiting_rebind_at` is within the
    /// TTL. A ghost row is excluded, as in `find_session_by_pane`.
    pub fn sessions_awaiting_rebind(&self, host_alias: &str) -> Result<Vec<SessionRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_alias = ?1 AND awaiting_rebind_at >= ?2 AND status != 'ghost'"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![host_alias, now_unix() - AWAITING_REBIND_TTL_SECS],
            map_session_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The session's conversations, newest first, capped at `limit`.
    pub fn list_conversations(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "{CONVERSATION_SELECT} WHERE c.session_id = ?1 \
             ORDER BY c.started_at DESC, c.id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit], map_conversation)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One conversation of the session by its Claude id, if it has one.
    pub fn get_conversation(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<Option<ConversationRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "{CONVERSATION_SELECT} WHERE c.session_id = ?1 AND c.claude_session_id = ?2"
                ),
                rusqlite::params![session_id, claude_session_id],
                map_conversation,
            )
            .optional()?)
    }

    /// The classification nudge (work graph M4.6) was already handed to
    /// this conversation. An unknown conversation reads as not nudged.
    pub fn conversation_nudged(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT classify_nudged_at IS NOT NULL FROM conversations \
                 WHERE session_id = ?1 AND claude_session_id = ?2",
                rusqlite::params![session_id, claude_session_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Stamp the nudge on the conversation. `true` only for the first stamp,
    /// so two racing prompts cannot both carry it.
    pub fn mark_conversation_nudged(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE conversations SET classify_nudged_at = ?3 \
             WHERE session_id = ?1 AND claude_session_id = ?2 AND classify_nudged_at IS NULL",
            rusqlite::params![session_id, claude_session_id, super::now_unix()],
        )?;
        Ok(n > 0)
    }

    /// Count one turn of the conversation.
    pub fn conversation_bump_turns(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE conversations SET turns = turns + 1 \
             WHERE session_id = ?1 AND claude_session_id = ?2",
            rusqlite::params![session_id, claude_session_id],
        )?;
        Ok(())
    }

    /// First 200 chars of the conversation's first prompt; later calls no-op.
    pub fn conversation_set_first_prompt(
        &self,
        session_id: i64,
        claude_session_id: &str,
        prompt: &str,
    ) -> Result<(), IpcError> {
        let p: String = prompt.chars().take(200).collect();
        self.conn.execute(
            "UPDATE conversations SET first_prompt = ?3 \
             WHERE session_id = ?1 AND claude_session_id = ?2 AND first_prompt IS NULL",
            rusqlite::params![session_id, claude_session_id, p],
        )?;
        Ok(())
    }

    /// Count one compaction and, when it is the current conversation, mark
    /// the context stale. `false` when a compaction was already recorded
    /// within [`COMPACT_DEDUPE_SECS`].
    pub fn conversation_record_compaction(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> Result<bool, IpcError> {
        let now = now_unix();
        let n = self.conn.execute(
            "UPDATE conversations SET compactions = compactions + 1, last_compact_at = ?3 \
             WHERE session_id = ?1 AND claude_session_id = ?2 \
               AND (last_compact_at IS NULL OR last_compact_at < ?3 - ?4)",
            rusqlite::params![session_id, claude_session_id, now, COMPACT_DEDUPE_SECS],
        )?;
        // A late signal for an ended conversation must not stale the size of
        // the one that replaced it.
        if n > 0 {
            let stale = self.conn.execute(
                "UPDATE sessions SET context_stale = 1 \
                 WHERE id = ?1 AND claude_session_id = ?2",
                rusqlite::params![session_id, claude_session_id],
            )?;
            if stale > 0 {
                self.emit_session(session_id)?;
            }
        }
        Ok(n > 0)
    }

    /// Write a context size for `claude_session_id` — ignored when that is no
    /// longer the session's current conversation. Derives `context_pct`.
    pub fn set_context(
        &self,
        session_id: i64,
        claude_session_id: &str,
        tokens: i64,
        window: i64,
        source: &str,
        model: Option<&str>,
    ) -> Result<Option<SessionRow>, IpcError> {
        let window = window.max(1);
        let pct = ((tokens as f64) * 100.0 / (window as f64)).round();
        let n = self.conn.execute(
            "UPDATE sessions SET context_tokens = ?3, context_window = ?4, context_pct = ?5, \
                 context_source = ?6, context_at = ?7, context_stale = 0, \
                 model = COALESCE(?8, model) \
             WHERE id = ?1 AND claude_session_id = ?2",
            rusqlite::params![
                session_id,
                claude_session_id,
                tokens,
                window,
                pct,
                source,
                now_unix(),
                model
            ],
        )?;
        if n == 0 {
            return Ok(None);
        }
        Ok(self.emit_session(session_id)?)
    }

    /// Flag the session's context size as out of date (compaction, resume).
    pub fn mark_context_stale(&self, session_id: i64) -> Result<Option<SessionRow>, IpcError> {
        self.conn.execute(
            "UPDATE sessions SET context_stale = 1 WHERE id = ?1",
            [session_id],
        )?;
        Ok(self.emit_session(session_id)?)
    }

    /// `start_source` of the session's current open conversation.
    pub fn current_conversation_source(&self, session_id: i64) -> Result<Option<String>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT c.start_source FROM conversations c JOIN sessions s ON s.id = c.session_id \
                 WHERE c.session_id = ?1 AND c.claude_session_id = s.claude_session_id",
                [session_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::store_with_recorder;
    use super::*;

    const A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

    fn session(s: &Store) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session("s", "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// #148 finding 3: the hub's list tools null-strip every `Option` key
    /// (`ok_json_compact`) before a hub client's desktop deserialises the
    /// reply straight into this struct (`session_conversations`) — see
    /// `src-tauri/src/backend/contract.rs`. Without `#[serde(default)]` on
    /// every `Option` field, a conversation whose `transcript_path` (etc.) is
    /// genuinely null fails to parse instead of reading back as `None`.
    #[test]
    fn a_conversation_row_with_its_option_fields_omitted_still_parses() {
        let json = serde_json::json!({
            "id": 1,
            "session_id": 2,
            "claude_session_id": A,
            "started_at": 100,
            "start_source": "startup",
            "turns": 0,
            "compactions": 0,
            "current": true,
        });
        let row: ConversationRow = serde_json::from_value(json)
            .expect("omitted Option keys must default, not fail to parse");
        assert_eq!(row.transcript_path, None);
        assert_eq!(row.ended_at, None);
        assert_eq!(row.end_reason, None);
        assert_eq!(row.model, None);
        assert_eq!(row.first_prompt, None);
    }

    #[test]
    fn clear_opens_a_new_conversation_and_resets_context() {
        let (s, bus) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        s.set_context(id, A, 90_000, 200_000, "transcript", Some("claude-opus-5"))
            .unwrap();
        let row = s
            .rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(row.claude_session_id.as_deref(), Some(B));
        assert_eq!(row.context.context_tokens, Some(0));
        assert_eq!(row.context_pct, Some(0.0));
        assert_eq!(row.context.context_source.as_deref(), Some("hook"));
        let convs = s.list_conversations(id, 10).unwrap();
        assert_eq!(convs.len(), 2);
        assert_eq!(
            (convs[0].claude_session_id.as_str(), convs[0].current),
            (B, true)
        );
        assert_eq!(convs[1].end_reason.as_deref(), Some("replaced"));
        assert!(convs[1].ended_at.is_some());
        assert!(bus.names().contains(&"session:conversations"));
    }

    #[test]
    fn a_ghost_row_is_never_awaiting_a_rebind() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.mark_awaiting_rebind(id).unwrap();
        assert_eq!(s.sessions_awaiting_rebind("local").unwrap().len(), 1);
        s.conn
            .execute("UPDATE sessions SET status = 'ghost' WHERE id = ?1", [id])
            .unwrap();
        assert!(s.sessions_awaiting_rebind("local").unwrap().is_empty());
    }

    #[test]
    fn close_reason_from_session_end_wins_over_replaced() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        s.close_conversation(id, A, "clear").unwrap();
        s.rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap();
        let convs = s.list_conversations(id, 10).unwrap();
        assert_eq!(convs[1].end_reason.as_deref(), Some("clear"));
    }

    #[test]
    fn resume_back_reopens_without_duplicate_and_marks_stale() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        s.rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap();
        let row = s
            .rebind_conversation(id, A, StartSource::Resume, None, None)
            .unwrap()
            .unwrap();
        assert!(row.context.context_stale);
        let convs = s.list_conversations(id, 10).unwrap();
        assert_eq!(convs.len(), 2);
        let a = convs.iter().find(|c| c.claude_session_id == A).unwrap();
        assert!(a.current && a.ended_at.is_none() && a.end_reason.is_none());
    }

    #[test]
    fn rebind_clears_a_transcript_path_of_another_id() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(
            id,
            A,
            StartSource::Fleet,
            Some("/h/.claude/projects/x/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"),
            None,
        )
        .unwrap();
        s.rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap();
        assert_eq!(s.session_transcript_path(id).unwrap(), None);
    }

    #[test]
    fn set_context_ignores_a_stale_conversation() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        s.rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap();
        s.set_context(id, A, 150_000, 200_000, "transcript", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.context.context_tokens, Some(0));
    }

    #[test]
    fn compaction_is_deduped_within_ten_seconds() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        assert!(s.conversation_record_compaction(id, A).unwrap());
        assert!(!s.conversation_record_compaction(id, A).unwrap());
        assert_eq!(s.list_conversations(id, 1).unwrap()[0].compactions, 1);
    }

    #[test]
    fn late_compaction_of_an_ended_conversation_leaves_the_current_context_fresh() {
        let (s, _) = store_with_recorder();
        let id = session(&s);
        s.rebind_conversation(id, A, StartSource::Fleet, None, None)
            .unwrap();
        s.rebind_conversation(id, B, StartSource::Clear, None, None)
            .unwrap();
        s.set_context(id, B, 10_000, 200_000, "transcript", None)
            .unwrap();
        assert!(s.conversation_record_compaction(id, A).unwrap());
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(
            !row.context.context_stale,
            "A is not current; B's size stands"
        );
        assert!(s.conversation_record_compaction(id, B).unwrap());
        assert!(
            s.get_session_by_id(id)
                .unwrap()
                .unwrap()
                .context
                .context_stale
        );
    }

    #[test]
    fn event_insert_pushes_session_event() {
        let (s, bus) = store_with_recorder();
        let id = session(&s);
        s.insert_session_event_for(id, Some(A), "compact_done", Some("auto"))
            .unwrap();
        assert!(bus.names().contains(&"session:event"));
        let ev = &s.list_session_events(id, 1).unwrap()[0];
        assert_eq!(ev.claude_session_id.as_deref(), Some(A));
    }
}
