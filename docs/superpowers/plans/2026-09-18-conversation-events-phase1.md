# Conversation Event Tracking — Phase 1 (Backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fleet tracks each Claude Code conversation inside a session: `/clear`, `/resume`, `/compact` rebind the row within a second, context resets to 0 on `/clear`, and every conversation is listed per session.

**Architecture:** Hooks resolve their row by tmux pane id (`X-Fleet-Pane`) before `claude_session_id`. A new `conversations` table holds one row per conversation; a single store function `rebind_conversation` owns every `claude_session_id` change (hooks, reconcile fallback, fleet create/recreate/move). Context size is computed from the transcript's last `usage` and takes precedence over the pane footer. New timeline events are pushed on the bus.

**Tech Stack:** Rust (fleet-core: rusqlite, axum, tokio, serde), Tauri 2 command layer, TypeScript wire types.

**Spec:** `docs/superpowers/specs/2026-09-18-conversation-events-design.md` (Phase 1 sections 1.1–1.8, Edge cases, Testing)

## Global Constraints

- Every value interpolated into a shell string goes through `crate::shell::quote` (`shq`).
- Never hold the `Mutex<Store>` guard across an `.await`.
- Timeline writes are best-effort (`let _ =` / `best_effort_event`) and never fail the mutation.
- Hooks always answer 2xx for a no-op; unknown rows are a no-op.
- Timestamps are unix **seconds** (`store::rows::now_unix()`), stored as `INTEGER`.
- Wire fields are snake_case; every new Rust `Option<T>` field is mirrored in TS as `T | null`.
- After adding any Tauri command or MCP tool: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- Branch: `feature/conversation-event-tracking-df4955` (this worktree). All git commands via `git -C <worktree>`. No `git stash`, pull, rebase or checkout by subagents.
- Frontend commands: `npx vitest run`, `npx svelte-check` (not `pnpm test`).
- A task is done only when the **full** `cargo test -p fleet-core` passes, run unpiped.

## File Map

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/034_conversations.sql` (new) | `conversations` table, new `sessions` / `session_events` columns, backfill |
| `crates/fleet-core/src/store/schema.rs` | Register migration 34 with an `already_applied` guard |
| `crates/fleet-core/src/store/conversations.rs` (new) | `ConversationRow`, open/close/list/bump, `rebind_conversation`, context setters |
| `crates/fleet-core/src/store/rows.rs` | `SessionContext` (flattened into `SessionRow`), `SESSION_COLUMNS`, mapper, `SessionEvent.claude_session_id` |
| `crates/fleet-core/src/store/timeline.rs` | `insert_session_event_for` (with claude id) + push `SessionEventAdded` |
| `crates/fleet-core/src/store/sessions.rs` | `set_claude_session_id` → delegates to rebind; `tmux_pane_id` lookup |
| `crates/fleet-core/src/store/reconcile.rs` | upsert: transcript reset on id change, context precedence, `tmux_pane_id` |
| `crates/fleet-core/src/events.rs` | `RowChange::SessionEventAdded`, `RowChange::ConversationsChanged` |
| `crates/fleet-core/src/mcp/hooks.rs` | Payload fields; read `X-Fleet-Pane` |
| `crates/fleet-core/src/service/hooks.rs` | `HookContext`, `resolve_hook_row`, new event handlers, rebind |
| `crates/fleet-core/src/service/context.rs` (new) | `context_from_jsonl`, `context_window_for`, `refresh_context` |
| `crates/fleet-core/src/service/hooks_install.rs` | New event set, pane header, SessionStart command entry, headers file |
| `crates/fleet-core/src/service/provision.rs` | Write `~/.claude/fleet-hook.headers` on the host |
| `crates/fleet-core/src/tmux.rs` | `#{pane_id}` in list-sessions; `TmuxSession.pane_id` |
| `crates/fleet-core/src/service/sessions/reconcile.rs` | Pass pane id; fallback rebind after write |
| `crates/fleet-core/src/service/tasks.rs` | Don't fail a task on a `/clear`/`/resume` rebind |
| `crates/fleet-core/src/service/transcript.rs` | `claude_session_id` override, context in `Conversation` |
| `crates/fleet-core/src/mcp/tools/messaging.rs` | MCP `session_conversations` |
| `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` | Tauri `session_conversations`; `session_conversation` arg |
| `src/lib/types.ts` (or wherever `SessionRow` lives), `src/lib/conversation.ts`, `src/lib/events.ts` | TS mirrors |
| `docs/control-api-reference.md` | Regenerated |

---

### Task 1: Migration 034 and the `SessionContext` wire fields

**Files:**
- Create: `crates/fleet-core/migrations/034_conversations.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (append to `MIGRATIONS`, add guard fn)
- Modify: `crates/fleet-core/src/store/rows.rs` (`SessionRow`, `SESSION_COLUMNS`, `map_session_row`, `SessionEvent`)
- Modify: every `SessionRow { … }` literal: `service/playbooks.rs:318`, `service/health.rs:164`, `service/gc.rs:368`, `service/sessions/tests.rs:143`, `service/account_usage_poll.rs` (4 sites) — find them with `grep -rn "usage: Default::default()" crates/fleet-core/src`
- Modify: `crates/fleet-core/src/store/timeline.rs` (`list_session_events` reads the new column)
- Modify: TS `SessionRow` interface (find with `grep -rn "usage_updated_at" src/lib/*.ts`)

**Interfaces:**
- Produces: `SessionContext { model, context_tokens, context_window, context_source, context_at, context_stale, tmux_pane_id }` flattened into `SessionRow` as `row.context`; `SessionEvent.claude_session_id: Option<String>`; table `conversations`.

- [ ] **Step 1: Write the migration**

`crates/fleet-core/migrations/034_conversations.sql`:

```sql
-- Conversation tracking (spec 2026-09-18-conversation-events-design.md).
--
-- conversations            one row per Claude Code conversation a session
--                          has run. At most one open row (ended_at IS NULL)
--                          per session: the one whose claude_session_id
--                          equals sessions.claude_session_id.
-- sessions.tmux_pane_id    the pane id (%17) reconcile last saw; hooks carry
--                          $TMUX_PANE in X-Fleet-Pane and resolve by it.
-- sessions.awaiting_rebind_at  set by SessionEnd(clear|resume): the next
--                          SessionStart / UserPromptSubmit from an unknown id
--                          in the same cwd on the same host rebinds this row.
-- sessions.context_*       context size of the current conversation.
--                          context_source: transcript | hook | pane.
-- sessions.model           model of the current conversation.
-- session_events.claude_session_id  the conversation an event belongs to.
CREATE TABLE IF NOT EXISTS conversations (
  id                INTEGER PRIMARY KEY,
  session_id        INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  claude_session_id TEXT    NOT NULL,
  transcript_path   TEXT,
  started_at        INTEGER NOT NULL,
  ended_at          INTEGER,
  start_source      TEXT    NOT NULL,
  end_reason        TEXT,
  model             TEXT,
  first_prompt      TEXT,
  turns             INTEGER NOT NULL DEFAULT 0,
  compactions       INTEGER NOT NULL DEFAULT 0,
  last_compact_at   INTEGER,
  UNIQUE (session_id, claude_session_id)
);
CREATE INDEX IF NOT EXISTS conversations_by_session
  ON conversations(session_id, started_at DESC);

ALTER TABLE sessions ADD COLUMN tmux_pane_id TEXT;
ALTER TABLE sessions ADD COLUMN awaiting_rebind_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN context_window INTEGER;
ALTER TABLE sessions ADD COLUMN context_source TEXT;
ALTER TABLE sessions ADD COLUMN context_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_stale INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN model TEXT;
ALTER TABLE session_events ADD COLUMN claude_session_id TEXT;

INSERT OR IGNORE INTO conversations (session_id, claude_session_id, transcript_path,
                                     started_at, start_source)
  SELECT id, claude_session_id, transcript_path, created_at, 'unknown'
  FROM sessions WHERE claude_session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (34);
```

- [ ] **Step 2: Register it with a guard** (the `ALTER TABLE`s are not re-runnable)

In `store/schema.rs`, next to `asset_inventory_has_managed`:

```rust
/// `already_applied` guard of migration 034: `sessions` already has its
/// `tmux_pane_id` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
fn sessions_has_tmux_pane_id(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'tmux_pane_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

Append to `MIGRATIONS`:

```rust
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 34,
        sql: include_str!("../../migrations/034_conversations.sql"),
        already_applied: Some(sessions_has_tmux_pane_id),
    },
```

- [ ] **Step 3: Add `SessionContext` to `rows.rs`**

After `SessionUsage`:

```rust
/// Current-conversation state (migration 034). Flattened into `SessionRow`
/// on the wire, so the field names are the wire names.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SessionContext {
    /// Model of the current conversation (SessionStart / transcript).
    pub model: Option<String>,
    /// Prompt size of the latest request: input + cache read + cache write.
    pub context_tokens: Option<i64>,
    /// Context window of `model` (200 000 or 1 000 000).
    pub context_window: Option<i64>,
    /// `transcript` | `hook` | `pane`: who wrote the context value last.
    pub context_source: Option<String>,
    /// Unix secs of the last context write.
    pub context_at: Option<i64>,
    /// True after a compaction or resume until the next usage line.
    pub context_stale: bool,
    /// tmux pane id (`%17`) reconcile last saw for this row.
    pub tmux_pane_id: Option<String>,
}
```

In `SessionRow`, after `usage`:

```rust
    /// Current-conversation context (migration 034), flattened on the wire.
    #[serde(flatten)]
    pub context: SessionContext,
```

Extend `SESSION_COLUMNS` (append after `usage_updated_at`):

```rust
     usage_cost_micros, usage_model, usage_updated_at, \
     model, context_tokens, context_window, context_source, context_at, context_stale, tmux_pane_id";
```

Extend `map_session_row` after `usage: SessionUsage { … },`:

```rust
        context: SessionContext {
            model: row.get(44)?,
            context_tokens: row.get(45)?,
            context_window: row.get(46)?,
            context_source: row.get(47)?,
            context_at: row.get(48)?,
            context_stale: row.get::<_, i64>(49)? != 0,
            tmux_pane_id: row.get(50)?,
        },
```

Add `context: Default::default(),` next to every `usage: Default::default(),` literal found by the grep in *Files*.

Add to `SessionEvent` (rows.rs:453):

```rust
    /// The conversation this event belongs to (migration 034); `None` for
    /// events not tied to one (ops, reconcile transitions).
    pub claude_session_id: Option<String>,
```

and in `store/timeline.rs::list_session_events` select `claude_session_id` as column 5 and map `claude_session_id: row.get(5)?`. Fix any other `SessionEvent { … }` literal (`grep -rn "SessionEvent {" crates src-tauri`).

- [ ] **Step 4: Write the migration tests** in `store/schema.rs`'s test module

```rust
    #[test]
    fn migration_034_backfills_one_open_conversation_per_bound_session() {
        let conn = Connection::open_in_memory().unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= 33) {
            let _ = version;
            conn.execute_batch(sql).unwrap();
        }
        conn.execute("INSERT INTO hosts (alias) VALUES ('local')", []).unwrap();
        conn.execute(
            "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status, claude_session_id)
             VALUES ('s', 'local', 7, 7, 'running', '11111111-1111-1111-1111-111111111111')",
            [],
        )
        .unwrap();
        let store = Store { conn, bus: Arc::new(crate::events::NoopEventBus) };
        store.migrate().unwrap();
        let (n, src, started): (i64, String, i64) = store
            .conn
            .query_row(
                "SELECT COUNT(*), start_source, started_at FROM conversations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((n, src.as_str(), started), (1, "unknown", 7));
    }

    #[test]
    fn session_row_carries_context_defaults() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store
            .upsert_session("s", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let row = store.get_session("s", "local").unwrap().unwrap();
        assert_eq!(row.context, SessionContext::default());
    }
```

If `hosts` has further NOT NULL columns at v33, copy the insert used by the existing seed test in the same module.

- [ ] **Step 5: Run and fix**

Run: `cargo test -p fleet-core store::schema`
Expected: PASS (including `migrate_is_idempotent`, the contiguous-versions test, and the new two).

- [ ] **Step 6: Mirror the wire type in TS**

In the TS `SessionRow` interface add:

```ts
  model: string | null;
  context_tokens: number | null;
  context_window: number | null;
  context_source: 'transcript' | 'hook' | 'pane' | null;
  context_at: number | null;
  context_stale: boolean;
  tmux_pane_id: string | null;
```

Fix TS fixtures that build a full `SessionRow` (`npx svelte-check` lists them). Run `npx svelte-check` and `npx vitest run`: PASS.

- [ ] **Step 7: Full suite + commit**

Run: `cargo test -p fleet-core` (unpiped). Expected: all pass.

```bash
git -C "$WT" add crates/fleet-core/migrations/034_conversations.sql crates/fleet-core/src src
git -C "$WT" commit -m "feat(store): migration 034 — conversations table and session context columns"
```

---

### Task 2: Store — conversations, rebind, context setters, pushed events

**Files:**
- Create: `crates/fleet-core/src/store/conversations.rs` (+ `mod conversations;` in `store/mod.rs`)
- Modify: `crates/fleet-core/src/events.rs` (two `RowChange` variants + `EventBus` helpers)
- Modify: `crates/fleet-core/src/store/timeline.rs` (`insert_session_event_for`, emit)
- Modify: `crates/fleet-core/src/store/sessions.rs:350` (`set_claude_session_id`)
- Modify: `src/lib/events.ts` (accept the two new names; phase 2 consumes them)

**Interfaces:**
- Consumes: Task 1 schema.
- Produces:
  - `pub struct ConversationRow { id, session_id, claude_session_id: String, transcript_path: Option<String>, started_at: i64, ended_at: Option<i64>, start_source: String, end_reason: Option<String>, model: Option<String>, first_prompt: Option<String>, turns: i64, compactions: i64, current: bool }` (Serialize)
  - `pub enum StartSource { Startup, Resume, Clear, Compact, Fork, Fleet, Unknown }` with `as_str()` and `from_hook(&str) -> StartSource`, and `resets_context(self) -> bool` (true for Startup, Clear, Fleet)
  - `Store::rebind_conversation(&self, session_id: i64, claude_session_id: &str, source: StartSource, transcript_path: Option<&str>, model: Option<&str>) -> Result<Option<SessionRow>, IpcError>`
  - `Store::close_conversation(&self, session_id: i64, claude_session_id: &str, reason: &str) -> Result<(), IpcError>`
  - `Store::mark_awaiting_rebind(&self, session_id: i64) -> Result<(), IpcError>`
  - `Store::list_conversations(&self, session_id: i64, limit: i64) -> Result<Vec<ConversationRow>, IpcError>`
  - `Store::conversation_bump_turns(&self, session_id: i64, claude_session_id: &str) -> Result<(), IpcError>`
  - `Store::conversation_set_first_prompt(&self, session_id: i64, claude_session_id: &str, prompt: &str) -> Result<(), IpcError>`
  - `Store::conversation_record_compaction(&self, session_id: i64, claude_session_id: &str) -> Result<bool, IpcError>` (false when deduped within 10 s)
  - `Store::set_context(&self, session_id: i64, claude_session_id: &str, tokens: i64, window: i64, source: &str, model: Option<&str>) -> Result<Option<SessionRow>, IpcError>` (no-op when the id is no longer current)
  - `Store::mark_context_stale(&self, session_id: i64) -> Result<Option<SessionRow>, IpcError>`
  - `Store::current_conversation_source(&self, session_id: i64) -> Result<Option<String>, IpcError>`
  - `Store::insert_session_event_for(&self, session_id: i64, claude_session_id: Option<&str>, kind: &str, detail: Option<&str>) -> Result<(), IpcError>`
  - `RowChange::SessionEventAdded(SessionEvent)` → name `"session:event"`; `RowChange::ConversationsChanged(i64)` → name `"session:conversations"`, payload `{ "session_id": n }`. Both under the existing `session` SSE kind.

- [ ] **Step 1: Write failing store tests** in `store/conversations.rs` `#[cfg(test)] mod tests`

```rust
use super::test_support::store_with_recorder;
use super::*;

const A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

fn session(s: &Store) -> i64 {
    s.upsert_host("local").unwrap();
    s.upsert_session("s", "local", None, None, 0, 0, "running", None).unwrap()
}

#[test]
fn clear_opens_a_new_conversation_and_resets_context() {
    let (s, bus) = store_with_recorder();
    let id = session(&s);
    s.rebind_conversation(id, A, StartSource::Fleet, None, None).unwrap();
    s.set_context(id, A, 90_000, 200_000, "transcript", Some("claude-opus-5")).unwrap();
    let row = s.rebind_conversation(id, B, StartSource::Clear, None, None).unwrap().unwrap();
    assert_eq!(row.claude_session_id.as_deref(), Some(B));
    assert_eq!(row.context.context_tokens, Some(0));
    assert_eq!(row.context_pct, Some(0.0));
    assert_eq!(row.context.context_source.as_deref(), Some("hook"));
    let convs = s.list_conversations(id, 10).unwrap();
    assert_eq!(convs.len(), 2);
    assert_eq!((convs[0].claude_session_id.as_str(), convs[0].current), (B, true));
    assert_eq!(convs[1].end_reason.as_deref(), Some("replaced"));
    assert!(convs[1].ended_at.is_some());
    assert!(bus.names().contains(&"session:conversations"));
}

#[test]
fn close_reason_from_session_end_wins_over_replaced() {
    let (s, _) = store_with_recorder();
    let id = session(&s);
    s.rebind_conversation(id, A, StartSource::Fleet, None, None).unwrap();
    s.close_conversation(id, A, "clear").unwrap();
    s.rebind_conversation(id, B, StartSource::Clear, None, None).unwrap();
    let convs = s.list_conversations(id, 10).unwrap();
    assert_eq!(convs[1].end_reason.as_deref(), Some("clear"));
}

#[test]
fn resume_back_reopens_without_duplicate_and_marks_stale() {
    let (s, _) = store_with_recorder();
    let id = session(&s);
    s.rebind_conversation(id, A, StartSource::Fleet, None, None).unwrap();
    s.rebind_conversation(id, B, StartSource::Clear, None, None).unwrap();
    let row = s.rebind_conversation(id, A, StartSource::Resume, None, None).unwrap().unwrap();
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
    s.rebind_conversation(id, A, StartSource::Fleet, Some("/h/.claude/projects/x/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"), None).unwrap();
    s.rebind_conversation(id, B, StartSource::Clear, None, None).unwrap();
    assert_eq!(s.session_transcript_path(id).unwrap(), None);
}

#[test]
fn set_context_ignores_a_stale_conversation() {
    let (s, _) = store_with_recorder();
    let id = session(&s);
    s.rebind_conversation(id, A, StartSource::Fleet, None, None).unwrap();
    s.rebind_conversation(id, B, StartSource::Clear, None, None).unwrap();
    s.set_context(id, A, 150_000, 200_000, "transcript", None).unwrap();
    let row = s.get_session_by_id(id).unwrap().unwrap();
    assert_eq!(row.context.context_tokens, Some(0));
}

#[test]
fn compaction_is_deduped_within_ten_seconds() {
    let (s, _) = store_with_recorder();
    let id = session(&s);
    s.rebind_conversation(id, A, StartSource::Fleet, None, None).unwrap();
    assert!(s.conversation_record_compaction(id, A).unwrap());
    assert!(!s.conversation_record_compaction(id, A).unwrap());
    assert_eq!(s.list_conversations(id, 1).unwrap()[0].compactions, 1);
}

#[test]
fn event_insert_pushes_session_event() {
    let (s, bus) = store_with_recorder();
    let id = session(&s);
    s.insert_session_event_for(id, Some(A), "compact_done", Some("auto")).unwrap();
    assert!(bus.names().contains(&"session:event"));
    let ev = &s.list_session_events(id, 1).unwrap()[0];
    assert_eq!(ev.claude_session_id.as_deref(), Some(A));
}
```

If `RecordingEventBus` has no `names()` helper, add one in `events.rs` (`pub fn names(&self) -> Vec<&'static str>` over the recorded `RowChange::name()`s) — check its existing API first and reuse what is there.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p fleet-core store::conversations`
Expected: compile errors (module / functions missing).

- [ ] **Step 3: Add the events**

In `events.rs` `RowChange`:

```rust
    /// A timeline event was appended (migration 013/034). Carries the row.
    SessionEventAdded(SessionEvent),
    /// A session's conversation list changed (opened / closed / reopened).
    /// Payload is the session id only; the UI refetches the small list.
    ConversationsChanged(i64),
```

`name()`: `RowChange::SessionEventAdded(_) => "session:event"`, `RowChange::ConversationsChanged(_) => "session:conversations"`.
`payload()`: `SessionEventAdded(e) => to_value(e)`, `ConversationsChanged(id) => serde_json::json!({ "session_id": id })`.
Import `SessionEvent` where `SessionRow` is imported. Make `SessionEvent` `Clone + Serialize` if it is not.

`EventBus` helpers:

```rust
    fn session_event_added(&self, e: &SessionEvent) {
        self.emit(&RowChange::SessionEventAdded(e.clone()));
    }
    fn conversations_changed(&self, session_id: i64) {
        self.emit(&RowChange::ConversationsChanged(session_id));
    }
```

- [ ] **Step 4: Timeline insert with claude id + push**

Replace the body of `insert_session_event` in `store/timeline.rs` with a delegation, and add the new function:

```rust
    pub fn insert_session_event(
        &self,
        session_id: i64,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        self.insert_session_event_for(session_id, None, kind, detail)
    }

    /// [`Self::insert_session_event`] tagged with the conversation it belongs
    /// to. Emits `session:event` with the inserted row.
    pub fn insert_session_event_for(
        &self,
        session_id: i64,
        claude_session_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<(), crate::ipc_error::IpcError> {
        let at = now_unix();
        self.conn.execute(
            "INSERT INTO session_events (session_id, at, kind, detail, claude_session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, at, kind, detail, claude_session_id],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM session_events WHERE session_id=?1 AND id NOT IN (\
                   SELECT id FROM session_events WHERE session_id=?1 \
                   ORDER BY at DESC, id DESC LIMIT ?2)",
            rusqlite::params![session_id, SESSION_EVENTS_CAP],
        )?;
        self.bus.session_event_added(&SessionEvent {
            id,
            session_id,
            at,
            kind: kind.to_string(),
            detail: detail.map(String::from),
            claude_session_id: claude_session_id.map(String::from),
        });
        Ok(())
    }
```

- [ ] **Step 5: Implement `store/conversations.rs`**

```rust
//! Per-session conversation tracking (migration 034). `rebind_conversation`
//! is the ONLY writer of `sessions.claude_session_id` outside reconcile's
//! first sighting; see the spec's §1.3.

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
        matches!(self, StartSource::Startup | StartSource::Clear | StartSource::Fleet)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ConversationRow {
    pub id: i64,
    pub session_id: i64,
    pub claude_session_id: String,
    pub transcript_path: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub start_source: String,
    pub end_reason: Option<String>,
    pub model: Option<String>,
    pub first_prompt: Option<String>,
    pub turns: i64,
    pub compactions: i64,
    /// True for the session's current conversation.
    pub current: bool,
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
    /// context and clear `current_activity` / `last_prompt`, resume marks the
    /// context stale. One transaction; emits `session:updated` and
    /// `session:conversations` after commit.
    pub fn rebind_conversation(
        &self,
        session_id: i64,
        claude_session_id: &str,
        source: StartSource,
        transcript_path: Option<&str>,
        model: Option<&str>,
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
        let path_sql = if same {
            "transcript_path = COALESCE(?3, transcript_path)"
        } else {
            "transcript_path = ?3"
        };
        let reset_sql = if source.resets_context() {
            ", context_tokens = 0, context_pct = 0, context_source = 'hook', \
               context_at = ?5, context_stale = 0, current_activity = NULL, last_prompt = NULL"
        } else if matches!(source, StartSource::Resume | StartSource::Unknown | StartSource::Fork)
            && !same
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
        tx.execute(
            &sql,
            rusqlite::params![session_id, claude_session_id, transcript_path, model, now],
        )?;
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

    /// SessionEnd(clear|resume): the next SessionStart / UserPromptSubmit
    /// from an unknown id in this row's cwd may rebind it (spec §1.2 step 3).
    pub fn mark_awaiting_rebind(&self, session_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE sessions SET awaiting_rebind_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        Ok(())
    }

    /// Rows on `host_alias` whose `awaiting_rebind_at` is within the TTL.
    pub fn sessions_awaiting_rebind(&self, host_alias: &str) -> Result<Vec<SessionRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_alias = ?1 AND awaiting_rebind_at >= ?2"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![host_alias, now_unix() - AWAITING_REBIND_TTL_SECS],
            map_session_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_conversations(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.session_id, c.claude_session_id, c.transcript_path, c.started_at, \
                    c.ended_at, c.start_source, c.end_reason, c.model, c.first_prompt, c.turns, \
                    c.compactions, \
                    (c.claude_session_id IS s.claude_session_id AND c.ended_at IS NULL) \
             FROM conversations c JOIN sessions s ON s.id = c.session_id \
             WHERE c.session_id = ?1 ORDER BY c.started_at DESC, c.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit], |r| {
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
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

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

    /// Count one compaction and mark the context stale. `false` when a
    /// compaction was already recorded within [`COMPACT_DEDUPE_SECS`].
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
        if n > 0 {
            self.mark_context_stale(session_id)?;
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
            rusqlite::params![session_id, claude_session_id, tokens, window, pct, source, now_unix(), model],
        )?;
        if n == 0 {
            return Ok(None);
        }
        Ok(self.emit_session(session_id)?)
    }

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
```

Add `pub use conversations::{ConversationRow, StartSource, AWAITING_REBIND_TTL_SECS};` in `store/mod.rs`.

- [ ] **Step 6: Route `set_claude_session_id` through the rebind**

`store/sessions.rs:350` — replace the body (production callers: `lifecycle.rs:666` create/recreate, `move_session.rs:1294`, `review.rs:88`; all are fleet-chosen ids):

```rust
    /// Record the Claude Code session id fleet launched this session with
    /// (create / recreate / move / review). Opens that conversation with
    /// source `fleet` via [`Self::rebind_conversation`], which closes any
    /// previous one as `replaced` and resets the context.
    pub fn set_claude_session_id(&self, id: i64, uuid: &str) -> Result<(), crate::ipc_error::IpcError> {
        self.rebind_conversation(id, uuid, StartSource::Fleet, None, None)?;
        Ok(())
    }
```

The return type changes from `rusqlite::Error` to `IpcError`; fix the three production call sites (they already log / `let _ =`) and test call sites compile unchanged (`.unwrap()`).

- [ ] **Step 7: TS event names**

In `src/lib/events.ts`, extend the `RowEvent` union:

```ts
  | { name: 'session:event'; payload: SessionEvent }
  | { name: 'session:conversations'; payload: { session_id: number } }
```

and make the dispatcher's `switch` ignore them for now (`case 'session:event': case 'session:conversations': break;`). Import/define `SessionEvent` from `src/lib/timeline.ts` and add `claude_session_id: string | null` to it.

- [ ] **Step 8: Run**

Run: `cargo test -p fleet-core` (full, unpiped). Expected: PASS. Then `npx svelte-check && npx vitest run`: PASS.

- [ ] **Step 9: Commit**

```bash
git -C "$WT" add -A crates/fleet-core/src src/lib
git -C "$WT" commit -m "feat(store): conversations — rebind, close, list, context setters; push timeline events"
```

---

### Task 3: Hook routing — pane header, row resolution, conversation events

**Files:**
- Modify: `crates/fleet-core/src/mcp/hooks.rs` (payload fields; `HeaderMap` → pane id)
- Modify: `crates/fleet-core/src/service/hooks.rs` (signature, resolver, handlers)
- Modify: `crates/fleet-core/src/store/sessions.rs` (`find_session_by_pane`)

**Interfaces:**
- Consumes: Task 2 store API.
- Produces:
  - `pub struct HookContext<'a> { pub caller: &'a Caller, pub pane_id: Option<String> }`
  - `pub fn apply_hook(store, ssh, payload: &HookPayload, ctx: &HookContext) -> Result<(), IpcError>` (was `caller: &Caller`)
  - `fn resolve_hook_row(s: &Store, payload: &HookPayload, ctx: &HookContext, may_rebind: bool) -> Result<Option<SessionRow>, IpcError>`
  - `Store::find_session_by_pane(&self, host_alias: &str, pane_id: &str) -> Result<Option<SessionRow>, IpcError>` (exactly one live row, else `None`)
  - Timeline kinds: `conversation_started`, `conversation_ended`, `compact_started`, `compact_done`, `turn_done`.

- [ ] **Step 1: Payload fields** in `mcp/hooks.rs` `HookPayload`:

```rust
    /// `SessionStart`: startup | resume | clear | compact | fork.
    pub source: Option<String>,
    /// `SessionStart`: the model id.
    pub model: Option<String>,
    /// `PreCompact` / `PostCompact`: manual | auto.
    pub trigger: Option<String>,
    /// `UserPromptSubmit`: the prompt text (first 200 chars stored as the
    /// conversation's `first_prompt`; never logged).
    pub prompt: Option<String>,
    /// `Stop`: the final assistant message (first 200 chars go to the
    /// `turn_done` timeline event; never logged).
    pub last_assistant_message: Option<String>,
```

- [ ] **Step 2: Read the pane header** in `handle_hook`:

```rust
pub async fn handle_hook(
    State(state): State<HookState>,
    Extension(caller): Extension<Caller>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<HookPayload>,
) -> StatusCode {
    …
    let pane_id = pane_header(&headers);
    let ctx = crate::service::hooks::HookContext { caller: &caller, pane_id };
    match crate::service::hooks::apply_hook(&state.store, &state.ssh, &payload, &ctx) {
```

```rust
/// `X-Fleet-Pane`: `$TMUX_PANE` of the hook's process (`%` + digits). Empty
/// (outside tmux, or a CLI that does not expand header env vars — it then
/// sends the literal `$TMUX_PANE`) or malformed → `None`.
pub fn pane_header(headers: &axum::http::HeaderMap) -> Option<String> {
    let v = headers.get("x-fleet-pane")?.to_str().ok()?.trim();
    let digits = v.strip_prefix('%')?;
    (!digits.is_empty() && digits.len() <= 10 && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| v.to_string())
}
```

Test (same file):

```rust
    #[test]
    fn pane_header_accepts_only_tmux_pane_ids() {
        let h = |v: &str| {
            let mut m = axum::http::HeaderMap::new();
            m.insert("x-fleet-pane", v.parse().unwrap());
            pane_header(&m)
        };
        assert_eq!(h("%17"), Some("%17".into()));
        assert_eq!(h(""), None);
        assert_eq!(h("$TMUX_PANE"), None);
        assert_eq!(h("%1;rm"), None);
        assert_eq!(pane_header(&axum::http::HeaderMap::new()), None);
    }
```

- [ ] **Step 3: `find_session_by_pane`** in `store/sessions.rs`:

```rust
    /// The one live row on `host_alias` whose last-seen pane is `pane_id`.
    /// `None` when there is none or more than one (a stale pane id after a
    /// tmux server restart shared with a new row).
    pub fn find_session_by_pane(
        &self,
        host_alias: &str,
        pane_id: &str,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_alias = ?1 AND tmux_pane_id = ?2 AND status != 'ghost' LIMIT 2"
        ))?;
        let rows: Vec<SessionRow> = stmt
            .query_map(rusqlite::params![host_alias, pane_id], map_session_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(match rows.len() {
            1 => rows.into_iter().next(),
            _ => None,
        })
    }
```

- [ ] **Step 4: Write failing hook tests** in `service/hooks.rs` tests (reuse `make_store`, `make_ssh`, `make_payload`; add a helper that sets the pane):

```rust
    const OLD: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

    fn pane_session(store: &Arc<Mutex<Store>>, name: &str, pane: &str) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        let id = s.upsert_session(name, "local", None, None, 0, 0, "running", None).unwrap();
        s.set_claude_session_id(id, OLD).unwrap();
        s.conn_ref()
            .execute("UPDATE sessions SET tmux_pane_id=?1 WHERE id=?2", rusqlite::params![pane, id])
            .unwrap();
        id
    }

    fn ctx<'a>(caller: &'a Caller, pane: Option<&str>) -> HookContext<'a> {
        HookContext { caller, pane_id: pane.map(String::from) }
    }

    #[test]
    fn session_start_clear_rebinds_by_pane_and_zeroes_context() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store.lock().unwrap().set_context(id, OLD, 120_000, 200_000, "transcript", None).unwrap();
        let mut p = make_payload("SessionStart", NEW);
        p.source = Some("clear".into());
        let host = Caller::for_host("local");
        apply_hook(&store, &make_ssh(), &p, &ctx(&host, Some("%3"))).unwrap();
        let row = store.lock().unwrap().get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_session_id.as_deref(), Some(NEW));
        assert_eq!(row.context.context_tokens, Some(0));
        let kinds: Vec<String> = store.lock().unwrap().list_session_events(id, 5).unwrap()
            .into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"conversation_started".to_string()));
    }

    #[test]
    fn old_stop_after_clear_does_not_rebind_back() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = Caller::for_host("local");
        let mut start = make_payload("SessionStart", NEW);
        start.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &start, &ctx(&host, Some("%3"))).unwrap();
        apply_hook(&store, &make_ssh(), &make_payload("Stop", OLD), &ctx(&host, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().claude_session_id.as_deref(), Some(NEW));
        let old = s.list_conversations(id, 5).unwrap().into_iter().find(|c| c.claude_session_id == OLD).unwrap();
        assert_eq!(old.turns, 1);
    }

    #[test]
    fn prompt_submit_rebinds_via_awaiting_mark_when_no_pane_header() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = Caller::for_host("local");
        let mut end = make_payload("SessionEnd", OLD);
        end.reason = Some("clear".into());
        apply_hook(&store, &make_ssh(), &end, &ctx(&host, None)).unwrap();
        // Status stays; conversation closed with reason clear.
        assert_ne!(store.lock().unwrap().get_session_by_id(id).unwrap().unwrap().claude_status.as_deref(), Some("stopped"));
        // One awaiting row on the host, no cwd on either side → rebinds.
        let mut prompt = make_payload("UserPromptSubmit", NEW);
        prompt.cwd = None;
        apply_hook(&store, &make_ssh(), &prompt, &ctx(&host, None)).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get_session_by_id(id).unwrap().unwrap().claude_session_id.as_deref(), Some(NEW));
        let convs = s.list_conversations(id, 5).unwrap();
        assert_eq!(convs.iter().find(|c| c.claude_session_id == OLD).unwrap().end_reason.as_deref(), Some("clear"));
    }

    #[test]
    fn master_caller_never_rebinds_through_the_awaiting_mark() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        store.lock().unwrap().mark_awaiting_rebind(id).unwrap();
        let master = Caller::master();
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", NEW), &ctx(&master, None)).unwrap();
        assert_eq!(store.lock().unwrap().get_session_by_id(id).unwrap().unwrap().claude_session_id.as_deref(), Some(OLD));
    }

    #[test]
    fn pre_and_post_compact_record_one_compaction() {
        let store = make_store();
        let id = pane_session(&store, "s", "%3");
        let host = Caller::for_host("local");
        let mut pre = make_payload("PreCompact", OLD);
        pre.trigger = Some("auto".into());
        apply_hook(&store, &make_ssh(), &pre, &ctx(&host, Some("%3"))).unwrap();
        assert_eq!(store.lock().unwrap().get_session_by_id(id).unwrap().unwrap().current_activity.as_deref(), Some("compacting"));
        apply_hook(&store, &make_ssh(), &make_payload("PostCompact", OLD), &ctx(&host, Some("%3"))).unwrap();
        let mut again = make_payload("SessionStart", OLD);
        again.source = Some("compact".into());
        apply_hook(&store, &make_ssh(), &again, &ctx(&host, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.list_conversations(id, 1).unwrap()[0].compactions, 1);
        assert!(s.get_session_by_id(id).unwrap().unwrap().context.context_stale);
    }

    #[test]
    fn pane_on_another_host_is_not_resolved() {
        let store = make_store();
        pane_session(&store, "s", "%3");
        store.lock().unwrap().upsert_host("other").unwrap();
        let other = Caller::for_host("other");
        let mut p = make_payload("SessionStart", NEW);
        p.source = Some("clear".into());
        apply_hook(&store, &make_ssh(), &p, &ctx(&other, Some("%3"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get_session("s", "local").unwrap().unwrap().claude_session_id.as_deref(), Some(OLD));
    }
```

Use the existing constructor for a host-bound `Caller` in this test module (grep `Caller {` / `Caller::` in the tests); if there is no `for_host`, build the struct literal the way existing tests do. Existing tests calling `apply_hook(…, &Caller::master())` change to `&HookContext { caller: &Caller::master(), pane_id: None }` — add a test helper `fn master_ctx() -> HookContext<'static>` using a `static` master caller, or update each call.

Resolver step 3 rule (write it in the resolver doc comment): match when exactly one awaiting row exists on the caller's host **and** either `payload.cwd` is absent, or the row's known cwd (worktree path, else project base path) is absent, or they are equal after `canonical_str`.

**Ambiguous id (observed live on mefistos: two rows share one `claude_session_id`).** Add `Store::sessions_by_claude_id(&self, id: &str) -> Result<Vec<SessionRow>, IpcError>` and make `host_checked_row` use it: exactly one row → as today; more than one → `None` (the id step abstains; only the pane step can resolve such a row). Test:

```rust
    #[test]
    fn an_id_shared_by_two_rows_resolves_only_by_pane() {
        let store = make_store();
        let a = pane_session(&store, "a", "%3");
        let b = pane_session(&store, "b", "%4"); // both bound to OLD
        let host = Caller::for_host("local");
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", OLD), &ctx(&host, None)).unwrap();
        let s = store.lock().unwrap();
        for id in [a, b] {
            assert_ne!(s.get_session_by_id(id).unwrap().unwrap().claude_status.as_deref(), Some("working"));
        }
        drop(s);
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", OLD), &ctx(&host, Some("%4"))).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get_session_by_id(b).unwrap().unwrap().claude_status.as_deref(), Some("working"));
        assert_ne!(s.get_session_by_id(a).unwrap().unwrap().claude_status.as_deref(), Some("working"));
    }
```

Note the `record_*_hook` store writes key on `claude_session_id` and would update BOTH rows. Give each of `record_stop_hook`, `record_prompt_submit_hook`, `record_session_end_hook`, `record_stop_failure_hook`, `record_notification_hook` a row-id variant (`…_for_row(&self, row_id: i64, …)`, `WHERE id = ?1`) and call those from the handlers with the resolved `row.id`; keep the old names as thin wrappers only if other callers need them (grep first).

- [ ] **Step 5: Run to see them fail**

Run: `cargo test -p fleet-core service::hooks`
Expected: compile errors, then failures.

- [ ] **Step 6: Implement the resolver and handlers** in `service/hooks.rs`

```rust
/// Who sent a hook and from which tmux pane.
pub struct HookContext<'a> {
    pub caller: &'a Caller,
    /// `X-Fleet-Pane` (validated `%N`), `None` outside tmux / old CLIs.
    pub pane_id: Option<String>,
}

/// Events that may move a row onto a new conversation id. Everything else
/// carrying a non-current id only updates the conversation it names
/// (spec: "/clear mid-turn").
fn may_rebind(event: &str) -> bool {
    matches!(event, "SessionStart" | "UserPromptSubmit")
}

/// Find the row a hook is about (spec §1.2):
/// 1. caller host + pane id, 2. `claude_session_id`, 3. (rebinding events,
/// host callers only) the single row on the host awaiting a rebind whose cwd
/// agrees. Host binding is enforced for the id path as before.
fn resolve_hook_row(
    s: &Store,
    payload: &HookPayload,
    ctx: &HookContext,
    may_rebind: bool,
) -> Result<Option<SessionRow>, IpcError> {
    if let (Some(host), Some(pane)) = (&ctx.caller.host_alias, &ctx.pane_id) {
        if let Some(row) = s.find_session_by_pane(host, pane)? {
            return Ok(Some(row));
        }
    }
    let Some(id) = payload.session_id.as_deref() else {
        return Ok(None);
    };
    if let Some(row) = host_checked_row(s, id, ctx.caller)? {
        return Ok(Some(row));
    }
    let Some(host) = ctx.caller.host_alias.as_deref() else {
        return Ok(None);
    };
    if !may_rebind {
        return Ok(None);
    }
    let awaiting = s.sessions_awaiting_rebind(host)?;
    let matching: Vec<SessionRow> = awaiting
        .into_iter()
        .filter(|r| match (payload.cwd.as_deref(), row_cwd(s, r)) {
            (Some(a), Some(b)) => canonical_str(a) == canonical_str(&b),
            _ => true,
        })
        .collect();
    Ok(if matching.len() == 1 { matching.into_iter().next() } else { None })
}

/// The row's known cwd: its worktree path, else its project's base path.
fn row_cwd(s: &Store, row: &SessionRow) -> Option<String> {
    row.worktree_id
        .and_then(|w| s.worktree_path(w).ok().flatten())
        .or_else(|| row.project_id.and_then(|p| s.project_base_path(p).ok().flatten()))
}

/// Resolve, then — for a rebinding event whose id differs from the row's —
/// move the row onto the payload's conversation. Returns the (possibly
/// rebound) row and whether the payload's id is now the current one.
fn resolve_and_rebind(
    s: &Store,
    payload: &HookPayload,
    ctx: &HookContext,
    source: StartSource,
) -> Result<Option<(SessionRow, bool)>, IpcError> {
    let event = payload.hook_event_name.as_deref().unwrap_or("");
    let Some(id) = payload.session_id.as_deref() else {
        return Ok(None);
    };
    let rebind_ok = may_rebind(event);
    let Some(row) = resolve_hook_row(s, payload, ctx, rebind_ok)? else {
        return Ok(None);
    };
    if row.claude_session_id.as_deref() == Some(id) {
        return Ok(Some((row, true)));
    }
    if !rebind_ok {
        return Ok(Some((row, false)));
    }
    crate::validate::claude_session_id(id)
        .map_err(|e| IpcError::new(codes::E_VALIDATE, e.message))?;
    let path = payload
        .transcript_path
        .as_deref()
        .filter(|p| valid_transcript_path(p, id));
    let rebound = s
        .rebind_conversation(row.id, id, source, path, payload.model.as_deref())?
        .unwrap_or(row);
    best_effort_event_for(s, rebound.id, Some(id), "conversation_started", Some(source.as_str()));
    Ok(Some((rebound, true)))
}

fn best_effort_event_for(s: &Store, session_id: i64, claude_id: Option<&str>, kind: &str, detail: Option<&str>) {
    if let Err(e) = s.insert_session_event_for(session_id, claude_id, kind, detail) {
        tracing::warn!(session_id, kind, error = %e, "[hook] session_event insert failed");
    }
}
```

Change `best_effort_event(s, id, kind, detail)` to pass the payload's `session_id` through `best_effort_event_for` at every existing call site, so hook events carry their conversation.

New dispatch arms in `apply_hook`:

```rust
        Some("SessionStart") => apply_session_start_hook(store, payload, ctx),
        Some("PreCompact") => apply_pre_compact_hook(store, payload, ctx),
        Some("PostCompact") => apply_post_compact_hook(store, payload, ctx),
```

```rust
/// SessionStart (command hook, spec §1.1): opens / reopens a conversation.
/// `compact` keeps the id and records a compaction instead.
fn apply_session_start_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload, ctx: &HookContext) -> Result<(), IpcError> {
    let source = StartSource::from_hook(payload.source.as_deref().unwrap_or(""));
    let s = lock(store)?;
    let Some((row, current)) = resolve_and_rebind(&s, payload, ctx, source)? else {
        return Ok(());
    };
    let id = payload.session_id.as_deref().unwrap_or_default();
    if !current {
        return Ok(());
    }
    if source == StartSource::Compact {
        if s.conversation_record_compaction(row.id, id)? {
            best_effort_event_for(&s, row.id, Some(id), "compact_done", payload.trigger.as_deref());
        }
        return Ok(());
    }
    // Same id as before (fleet launched it with --session-id): the rebind was
    // skipped, so apply the source's effects here.
    if row.claude_session_id.as_deref() == Some(id) && source.resets_context() {
        s.rebind_conversation(row.id, id, source, payload.transcript_path.as_deref()
            .filter(|p| valid_transcript_path(p, id)), payload.model.as_deref())?;
    }
    Ok(())
}

fn apply_pre_compact_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload, ctx: &HookContext) -> Result<(), IpcError> {
    let s = lock(store)?;
    let Some((row, true)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    s.set_current_activity(row.id, Some("compacting"))?;
    best_effort_event_for(&s, row.id, payload.session_id.as_deref(), "compact_started", payload.trigger.as_deref());
    Ok(())
}

fn apply_post_compact_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload, ctx: &HookContext) -> Result<(), IpcError> {
    let s = lock(store)?;
    let Some((row, true)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    let id = payload.session_id.as_deref().unwrap_or_default();
    s.set_current_activity(row.id, None)?;
    if s.conversation_record_compaction(row.id, id)? {
        best_effort_event_for(&s, row.id, Some(id), "compact_done", payload.trigger.as_deref());
    }
    Ok(())
}
```

Note: the SessionStart branch where the id is the same — simplify: `resolve_and_rebind` returns early with `current = true` without rebinding; then for resetting sources call `rebind_conversation` (which handles `same` correctly: it keeps the path, resets context). Remove the redundant `row.claude_session_id == id` check if it is always true at that point.

Add `Store::set_current_activity(&self, id: i64, activity: Option<&str>) -> Result<Option<SessionRow>, IpcError>` in `store/sessions.rs` (UPDATE + `emit_session`) if no equivalent exists (`grep -n "current_activity = " crates/fleet-core/src/store/sessions.rs`).

Rewrite the existing handlers on top of `resolve_and_rebind`:

- `apply_prompt_submit_hook`: `resolve_and_rebind(…, StartSource::Unknown)`; when `current`: `record_prompt_submit_hook(id)`, `conversation_set_first_prompt(row.id, id, prompt)` when `payload.prompt` is non-empty; also clear `current_activity` if it is `compacting`.
- `apply_stop_hook`: resolve; when `!current` → `conversation_bump_turns(row.id, id)` and return (no status change); when `current` → existing body plus `conversation_bump_turns`, `turn_done` event (`detail` = first 200 chars of `last_assistant_message`), and spawn `crate::service::context::refresh_context(store, ssh, row.id)` (Task 4 adds it; in this task add the call behind a `// Task 4` stub function in `service/context.rs` that returns immediately, so the tree compiles).
- `apply_stop_failure_hook`: same current/non-current split; `bump_turns` in both.
- `apply_notification_hook`: only when `current`.
- `apply_session_end_hook`: accept every reason. `clear | resume` → `close_conversation(row.id, id, reason)`, `mark_awaiting_rebind(row.id)`, event `conversation_ended` (detail reason); status untouched. Other reasons → existing `stopped` path plus `close_conversation(row.id, id, reason)`. Delete `SESSION_END_REASONS`; update the test at ~line 1200 that asserted `clear` is a no-op to assert the new behaviour.
- Worktree hooks keep their current code (they resolve by caller host, not session).

`remember_transcript_path` stays for the `current` case.

- [ ] **Step 7: Run**

Run: `cargo test -p fleet-core service::hooks mcp::hooks` then the full `cargo test -p fleet-core`.
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git -C "$WT" commit -am "feat(hooks): resolve by tmux pane, rebind on SessionStart/UserPromptSubmit, track compaction and conversation end"
```

---

### Task 4: Context size from the transcript

**Files:**
- Create: `crates/fleet-core/src/service/context.rs` (+ `pub mod context;` in `service/mod.rs`)
- Modify: `crates/fleet-core/src/service/transcript.rs` (`Conversation.context`, `pub(crate)` access to `resolve_args` / `tail_script` / `read_tail`)

**Interfaces:**
- Consumes: `Store::set_context`, `transcript::resolve_args`, `transcript::tail_script`, `transcript::read_tail`.
- Produces:
  - `pub struct ContextUsage { pub tokens: i64, pub window: i64, pub model: Option<String> }` (Serialize, Clone, PartialEq, Debug)
  - `pub fn context_window_for(model: Option<&str>, tokens: i64) -> i64`
  - `pub fn context_from_jsonl(jsonl: &str) -> Option<ContextUsage>`
  - `pub async fn refresh_context(store: Arc<Mutex<Store>>, ssh: Arc<SshClient>, session_id: i64)`
  - `Conversation.context: Option<ContextView>` where `pub struct ContextView { pub tokens: i64, pub window: i64, pub pct: f64, pub stale: bool }`

- [ ] **Step 1: Failing tests** in `service/context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn asst(model: &str, input: i64, read: i64, write: i64, output: i64) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": { "model": model, "usage": {
                "input_tokens": input, "cache_read_input_tokens": read,
                "cache_creation_input_tokens": write, "output_tokens": output } }
        })
        .to_string()
    }

    #[test]
    fn last_assistant_usage_wins_and_output_is_excluded() {
        let jsonl = [
            asst("claude-opus-5", 10, 1_000, 0, 50),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            asst("claude-opus-5", 5, 40_000, 2_000, 900),
        ]
        .join("\n");
        let u = context_from_jsonl(&jsonl).unwrap();
        assert_eq!(u.tokens, 42_005);
        assert_eq!(u.window, 200_000);
        assert_eq!(u.model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn sidechain_and_synthetic_entries_are_ignored() {
        let side = serde_json::json!({"type":"assistant","isSidechain":true,
            "message":{"model":"m","usage":{"input_tokens":999_999}}}).to_string();
        let synth = serde_json::json!({"type":"assistant",
            "message":{"model":"<synthetic>","usage":{"input_tokens":0}}}).to_string();
        let jsonl = [asst("claude-sonnet-5", 100, 0, 0, 0), side, synth].join("\n");
        assert_eq!(context_from_jsonl(&jsonl).unwrap().tokens, 100);
    }

    #[test]
    fn compact_boundary_after_last_usage_yields_none() {
        let boundary = r#"{"type":"system","subtype":"compact_boundary"}"#;
        let jsonl = [asst("m", 150_000, 0, 0, 0), boundary.to_string()].join("\n");
        assert_eq!(context_from_jsonl(&jsonl), None);
    }

    #[test]
    fn window_detects_one_million_models() {
        assert_eq!(context_window_for(Some("claude-opus-5[1m]"), 10), 1_000_000);
        assert_eq!(context_window_for(Some("claude-sonnet-5"), 10), 200_000);
        assert_eq!(context_window_for(None, 250_000), 1_000_000);
        assert_eq!(context_window_for(None, 10), 200_000);
    }

    #[test]
    fn a_partial_leading_line_is_tolerated() {
        let jsonl = format!("ut_tokens\":1}}}}\n{}", asst("m", 7, 0, 0, 0));
        assert_eq!(context_from_jsonl(&jsonl).unwrap().tokens, 7);
    }
}
```

- [ ] **Step 2: Run to fail** — `cargo test -p fleet-core service::context` → compile error.

- [ ] **Step 3: Implement**

```rust
//! Context size of a conversation from its transcript (spec §1.5): the last
//! main-thread assistant entry's `usage` is the prompt size of the latest
//! request. The pane footer is a fallback only.

use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Tail read for a context refresh: enough for several assistant entries.
const CONTEXT_READ_BYTES: usize = 262_144;
pub const WINDOW_DEFAULT: i64 = 200_000;
pub const WINDOW_1M: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ContextUsage {
    pub tokens: i64,
    pub window: i64,
    pub model: Option<String>,
}

/// Window of `model`: `[1m]` models get 1M. A token count above the default
/// window proves a 1M window whatever the model id says.
pub fn context_window_for(model: Option<&str>, tokens: i64) -> i64 {
    if model.is_some_and(|m| m.contains("[1m]")) || tokens > WINDOW_DEFAULT {
        WINDOW_1M
    } else {
        WINDOW_DEFAULT
    }
}

/// The context usage after the transcript's last entry, or `None` when no
/// usage is known — none in the tail, or a compaction after the last one
/// (the size is unknown until the next reply).
pub fn context_from_jsonl(jsonl: &str) -> Option<ContextUsage> {
    let mut last: Option<ContextUsage> = None;
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system") if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") => {
                last = None;
            }
            Some("assistant") => {
                let msg = v.get("message");
                let model = msg.and_then(|m| m.get("model")).and_then(|m| m.as_str());
                if model == Some("<synthetic>") {
                    continue;
                }
                let Some(u) = msg.and_then(|m| m.get("usage")) else {
                    continue;
                };
                let n = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
                let tokens = n("input_tokens")
                    + n("cache_read_input_tokens")
                    + n("cache_creation_input_tokens");
                last = Some(ContextUsage {
                    tokens,
                    window: context_window_for(model, tokens),
                    model: model.map(String::from),
                });
            }
            _ => {}
        }
    }
    last
}

/// Re-read the current conversation's transcript tail and store its context
/// size (source `transcript`). Best-effort: every failure is logged at debug
/// and leaves the stored value. The store lock is never held across the read.
/// Retries once after 500 ms when the tail has no usage yet (the Stop hook
/// can land before the line is flushed).
pub async fn refresh_context(store: Arc<Mutex<Store>>, ssh: Arc<SshClient>, session_id: i64) {
    for attempt in 0..2 {
        if attempt == 1 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let row = match lock(&store).and_then(|s| Ok(s.get_session_by_id(session_id)?)) {
            Ok(Some(r)) => r,
            _ => return,
        };
        let Ok(args) = crate::service::transcript::resolve_args(&store, &row, 1, 1) else {
            return;
        };
        let claude_id = args.claude_session_id.clone();
        let text = match crate::service::transcript::read_tail_bytes(&args, CONTEXT_READ_BYTES, &ssh).await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(session_id, error = %e.message, "[context] tail read failed");
                return;
            }
        };
        if let Some(u) = context_from_jsonl(&text) {
            if let Ok(s) = lock(&store) {
                let _ = s.set_context(session_id, &claude_id, u.tokens, u.window, "transcript", u.model.as_deref());
            }
            return;
        }
    }
}
```

In `transcript.rs` add:

```rust
/// Read the last `max_bytes` of `args`' transcript (shared with
/// `service::context`). Errors as [`tail_script`] / [`read_tail`].
pub(crate) async fn read_tail_bytes(
    args: &TranscriptArgs,
    max_bytes: usize,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    let script = tail_script(args, max_bytes)?;
    read_tail(args, &script, ssh).await
}
```

Replace the Task 3 stub with this implementation; the Stop / StopFailure handlers spawn it:

```rust
        {
            let store = Arc::clone(store);
            let ssh = Arc::clone(ssh);
            let sid = row.id;
            crate::rt::spawn(async move {
                crate::service::context::refresh_context(store, ssh, sid).await;
            });
        }
```

(`apply_stop_failure_hook` needs the `ssh` argument now — pass it from `apply_hook`.)

- [ ] **Step 4: Context in `session_conversation`**

In `transcript.rs`:

```rust
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ContextView {
    pub tokens: i64,
    pub window: i64,
    pub pct: f64,
    pub stale: bool,
}
```

Add `pub context: Option<ContextView>` to `Conversation` (set `None` everywhere a `Conversation` is built; `fetch_conversation` sets it from `context_from_jsonl(&text)` with `stale: false`, `pct = (tokens*100/window).round()`). Test in `transcript.rs`:

```rust
    #[test]
    fn conversation_context_view_rounds_pct() {
        let u = crate::service::context::ContextUsage { tokens: 50_000, window: 200_000, model: None };
        let v = ContextView::from(&u);
        assert_eq!((v.pct, v.stale), (25.0, false));
    }
```

with `impl From<&ContextUsage> for ContextView`.

The Tauri `session_conversation` command (and the MCP twin) write the value back after the fetch: `s.set_context(row.id, &claude_id, v.tokens, v.window, "transcript", None)` — only when reading the **current** conversation (Task 7 adds the override arg).

- [ ] **Step 5: Run** full `cargo test -p fleet-core` → PASS. **Commit:**

```bash
git -C "$WT" commit -am "feat(context): compute context size from the transcript's last usage on Stop and on read"
```

---

### Task 5: Reconcile — pane ids, context precedence, fallback rebind

**Files:**
- Modify: `crates/fleet-core/src/tmux.rs` (`TmuxSession.pane_id`, both list-sessions formats at ~270 and ~465, `parse_sessions`)
- Modify: every `TmuxSession { … }` literal (`repair_tick.rs:1456`, `service/sessions/tests.rs` ×4) — add `pane_id: None`
- Modify: `crates/fleet-core/src/store/rows.rs` (`ReconcileSession.tmux_pane_id`)
- Modify: `crates/fleet-core/src/store/reconcile.rs` (`upsert_session_in_tx`)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs` (~450–545)

**Interfaces:**
- Consumes: `Store::rebind_conversation`, `StartSource::Unknown`.
- Produces: `TmuxSession.pane_id: Option<String>`, `ReconcileSession.tmux_pane_id: Option<String>`.

- [ ] **Step 1: Failing tmux parser test** (`tmux.rs` tests):

```rust
    #[test]
    fn parse_sessions_reads_an_optional_pane_id() {
        let out = "a|1|2|0|/w|%7\nb|1|2|1|/x\n";
        let s = parse_sessions(out);
        assert_eq!(s[0].pane_id.as_deref(), Some("%7"));
        assert_eq!(s[1].pane_id, None);
        // A 7th field is still a malformed line (a `|` in the name).
        assert!(parse_sessions("a|b|1|2|0|/w|%7").is_empty());
    }
```

- [ ] **Step 2: Implement**: add `pub pane_id: Option<String>` to `TmuxSession`; append `|#{pane_id}` to both `list-sessions -F` format strings; in `parse_sessions`:

```rust
            let path = it.next()?;
            let pane_id = it.next().filter(|p| p.starts_with('%')).map(String::from);
            if it.next().is_some() {
                return None;
            }
```

Hmm — a 6-field line whose 6th field does not start with `%` would be a name containing `|`; reject it: replace the `filter` with

```rust
            let pane_id = match it.next() {
                None => None,
                Some(p) if p.starts_with('%') => Some(p.to_string()),
                Some(_) => return None,
            };
```

Run `cargo test -p fleet-core tmux` → PASS.

- [ ] **Step 3: Failing store tests** (`store/reconcile.rs` tests, using `reconcile_one`-style helpers):

```rust
    #[test]
    fn a_reset_context_is_not_resurrected_by_an_empty_pane_footer() {
        let (mut s, _) = super::test_support::store_with_recorder();
        let row = super::test_support::reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.rebind_conversation(row.id, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", StartSource::Clear, None, None).unwrap();
        // pane footer shows nothing this pass
        let after = super::test_support::reconcile_one(&mut s, "a", Some("idle"), None, None);
        assert_eq!(after.context_pct, Some(0.0));
    }

    #[test]
    fn a_fresh_transcript_value_beats_a_pane_value() {
        let (mut s, _) = super::test_support::store_with_recorder();
        let row = super::test_support::reconcile_one(&mut s, "a", Some("idle"), None, None);
        let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        s.rebind_conversation(row.id, id, StartSource::Fleet, None, None).unwrap();
        s.set_context(row.id, id, 100_000, 200_000, "transcript", None).unwrap();
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession { tmux_name: "a", created_at: 1, last_activity_at: 1,
                context_pct: Some(12.0), intel_observed: true, ..Default::default() }],
            keep: &["a".to_string()],
            ..super::test_support::empty_probe("local", 1)
        }).unwrap();
        assert_eq!(s.get_session("a", "local").unwrap().unwrap().context_pct, Some(50.0));
    }

    #[test]
    fn a_pane_value_applies_when_no_other_source_wrote() {
        let (mut s, _) = super::test_support::store_with_recorder();
        super::test_support::reconcile_one(&mut s, "a", Some("idle"), None, None);
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession { tmux_name: "a", created_at: 1, last_activity_at: 1,
                context_pct: Some(12.0), tmux_pane_id: Some("%4".into()), intel_observed: true,
                ..Default::default() }],
            keep: &["a".to_string()],
            ..super::test_support::empty_probe("local", 1)
        }).unwrap();
        let row = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(row.context_pct, Some(12.0));
        assert_eq!(row.context.context_source.as_deref(), Some("pane"));
        assert_eq!(row.context.tmux_pane_id.as_deref(), Some("%4"));
    }

    #[test]
    fn an_id_change_from_claude_agents_clears_the_transcript_path() {
        let (mut s, _) = super::test_support::store_with_recorder();
        let row = super::test_support::reconcile_one(&mut s, "a", None, None, None);
        let old = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        s.rebind_conversation(row.id, old, StartSource::Fleet,
            Some("/h/.claude/projects/x/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"), None).unwrap();
        s.apply_host_reconcile(HostReconcile {
            sessions: &[ReconcileSession { tmux_name: "a", created_at: 1, last_activity_at: 1,
                claude_session_id: Some("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into()), ..Default::default() }],
            keep: &["a".to_string()],
            ..super::test_support::empty_probe("local", 1)
        }).unwrap();
        assert_eq!(s.session_transcript_path(row.id).unwrap(), None);
    }
```

- [ ] **Step 4: Implement the upsert changes** in `upsert_session_in_tx`:

1. New param `tmux_pane_id: Option<&str>` bound as `?21`; add `tmux_pane_id` to the INSERT column list and `?21` to VALUES; in the UPDATE: `tmux_pane_id=COALESCE(excluded.tmux_pane_id, tmux_pane_id),`.
2. Transcript path (SET clauses see the OLD row, so compare to the old id):

```sql
               transcript_path=CASE WHEN excluded.claude_session_id IS NOT NULL
                                     AND excluded.claude_session_id IS NOT claude_session_id
                                    THEN NULL ELSE transcript_path END,
```

3. Context precedence — replace `context_pct=COALESCE(excluded.context_pct, context_pct),` with:

```sql
               -- Pane footer is a fallback (spec §1.5): it applies only when no
               -- hook/transcript value exists or that value is > 120 s old,
               -- and a missing footer never overwrites anything.
               context_pct=CASE WHEN excluded.context_pct IS NULL THEN context_pct
                                WHEN context_source IN ('transcript','hook')
                                     AND context_at >= ?19 - 120 THEN context_pct
                                ELSE excluded.context_pct END,
               context_source=CASE WHEN excluded.context_pct IS NULL THEN context_source
                                   WHEN context_source IN ('transcript','hook')
                                        AND context_at >= ?19 - 120 THEN context_source
                                   ELSE 'pane' END,
               context_at=CASE WHEN excluded.context_pct IS NULL THEN context_at
                               WHEN context_source IN ('transcript','hook')
                                    AND context_at >= ?19 - 120 THEN context_at
                               ELSE ?19 END,
```

(`?19` is already `now_unix()`.) Thread `tmux_pane_id` from `ReconcileSession` through `apply_host_reconcile` (store/reconcile.rs ~379, where `sess.context_pct` is passed).

Add `pub tmux_pane_id: Option<String>,` to `ReconcileSession` (it derives `Default`).

- [ ] **Step 5: Fallback rebind** in `service/sessions/reconcile.rs`:

Extend the `priors` tuple with the prior `claude_session_id` (`prior.claude_session_id`), pass `tmux_pane_id: sess.pane_id.clone()` into `ReconcileSession`, and in the post-write loop:

```rust
                // Fallback rebind (spec §1.4): `claude agents` moved the row
                // onto another conversation (no hooks, or an old CLI). Open
                // it as `unknown`; the upsert already cleared the stale
                // transcript path.
                if let (Some(new_id), true) = (row.claude_session_id.as_deref(), row.claude_session_id != *old_claude_id) {
                    if let Err(e) = s.rebind_conversation(row.id, new_id, StartSource::Unknown, None, None) {
                        tracing::warn!(host = %host.alias, session = %tmux_name, error = %e, "[reconcile] conversation rebind failed");
                    } else {
                        let _ = s.insert_session_event_for(row.id, Some(new_id), "conversation_started", Some("unknown"));
                    }
                }
```

**Never bind one id to two rows.** In the same post-write loop, before the rebind: if another live row on the same host already has `new_id` as its `claude_session_id`, skip the rebind and log at `debug` (the cwd match in `claude_agents::find_for_session` is ambiguous; the hooks' pane binding will settle it). Enforce it in the upsert too: `claude_session_id=CASE WHEN excluded.claude_session_id IS NOT NULL AND EXISTS (SELECT 1 FROM sessions o WHERE o.claude_session_id = excluded.claude_session_id AND o.host_alias = excluded.host_alias AND o.tmux_name != excluded.tmux_name AND o.status != 'ghost') THEN claude_session_id ELSE COALESCE(excluded.claude_session_id, claude_session_id) END` (and gate the `transcript_path` reset on the same condition). Store test: two rows, row `a` bound to A; a pass reporting A for row `b` leaves `b`'s id unchanged.

Test in `service/reconcile_tests.rs` (follow the file's existing harness that feeds fake `claude agents` rows): a row bound to A, a reconcile pass whose agent row reports B for the same name → `list_conversations` has 2 rows, the current is B with `start_source = "unknown"`, A has `end_reason = "replaced"`.

- [ ] **Step 6: Run** full `cargo test -p fleet-core` → PASS (the MCP-1 guard tests in `store/reconcile.rs` must still pass). **Commit:**

```bash
git -C "$WT" commit -am "feat(reconcile): record tmux pane ids, pane context as fallback only, rebind conversations on id change"
```

---

### Task 6: Hook installation — pane header, new events, SessionStart command

**Files:**
- Modify: `crates/fleet-core/src/service/hooks_install.rs`
- Modify: `crates/fleet-core/src/service/provision.rs` (`provision_hook` writes the headers file)

**Interfaces:**
- Produces:
  - `pub const HOOK_HEADERS_FILE: &str = "fleet-hook.headers";`
  - `pub fn hook_headers_content(token: &str) -> String` → `"Authorization: Bearer {token}\n"`
  - `pub fn session_start_command(hook_url: &str) -> String`
  - `FLEET_HOOK_EVENTS` as `&[(&str, &str, HookKind)]` with `enum HookKind { Http, Command }`

- [ ] **Step 1: Failing tests** (`hooks_install.rs` tests):

```rust
    #[test]
    fn http_entries_carry_the_pane_header() {
        let e = hook_entry("http://127.0.0.1:4180/hook", "tok");
        assert_eq!(e["headers"]["X-Fleet-Pane"], "$TMUX_PANE");
        assert_eq!(e["allowedEnvVars"], serde_json::json!(["TMUX_PANE"]));
        assert!(is_fleet_hook_entry(&e));
    }

    #[test]
    fn session_start_is_a_tokenless_command_hook() {
        let merged = merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "sekrit").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let h = &v["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["async"], true);
        let cmd = h["command"].as_str().unwrap();
        assert!(cmd.contains("fleet-hook.headers"));
        assert!(cmd.contains("'http://127.0.0.1:4180/hook'"));
        assert!(!cmd.contains("sekrit"));
    }

    #[test]
    fn all_events_are_installed_once_and_remerge_is_idempotent() {
        let url = "http://127.0.0.1:4180/hook";
        let once = merge_hook_into_settings_json("", url, "t").unwrap();
        let twice = merge_hook_into_settings_json(&once, url, "t").unwrap();
        assert_eq!(once, twice);
        let v: serde_json::Value = serde_json::from_str(&once).unwrap();
        for ev in ["Stop", "UserPromptSubmit", "PostToolUse", "SessionEnd", "StopFailure",
                   "Notification", "SessionStart", "PreCompact", "PostCompact"] {
            assert_eq!(v["hooks"][ev].as_array().unwrap().len(), 1, "{ev}");
        }
        assert_eq!(v["hooks"]["SessionEnd"][0]["matcher"],
                   "logout|prompt_input_exit|other|clear|resume");
    }

    #[test]
    fn a_base_url_change_replaces_the_session_start_command() {
        let a = merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "t").unwrap();
        let b = merge_hook_into_settings_json(&a, "https://fleet.example.com/hook", "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["hooks"][0]["command"].as_str().unwrap().contains("fleet.example.com"));
    }

    #[test]
    fn a_user_session_start_hook_survives() {
        let user = r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"echo hi"}]}]}}"#;
        let merged = merge_hook_into_settings_json(user, "http://127.0.0.1:4180/hook", "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    }
```


- [ ] **Step 2: Implement**

```rust
/// Name of the file (in `~/.claude`, mode 0600) holding the bearer header the
/// SessionStart command hook sends with `curl -H @file`. Keeps the token out
/// of argv (SEC-3) and out of the command string in settings.json.
pub const HOOK_HEADERS_FILE: &str = "fleet-hook.headers";

pub fn hook_headers_content(token: &str) -> String {
    format!("Authorization: Bearer {token}\n")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Http,
    /// Events that accept only command / mcp_tool handlers (SessionStart).
    Command,
}

pub const SESSION_END_MATCHER: &str = "logout|prompt_input_exit|other|clear|resume";

pub const FLEET_HOOK_EVENTS: &[(&str, &str, HookKind)] = &[
    ("Stop", "", HookKind::Http),
    ("UserPromptSubmit", "", HookKind::Http),
    ("PostToolUse", WORKTREE_TOOL_MATCHER, HookKind::Http),
    ("SessionEnd", SESSION_END_MATCHER, HookKind::Http),
    ("StopFailure", "", HookKind::Http),
    ("Notification", NOTIFICATION_MATCHER, HookKind::Http),
    ("SessionStart", "", HookKind::Command),
    ("PreCompact", "", HookKind::Http),
    ("PostCompact", "", HookKind::Http),
];

pub fn hook_entry(hook_url: &str, token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": hook_url,
        "headers": {
            "Authorization": format!("Bearer {token}"),
            "X-Fleet-Pane": "$TMUX_PANE"
        },
        "allowedEnvVars": ["TMUX_PANE"],
        "timeout": HOOK_TIMEOUT_SECS
    })
}

/// The SessionStart command: POST the hook body from stdin with the token
/// header file and the pane id. Never fails the session start.
pub fn session_start_command(hook_url: &str) -> String {
    format!(
        "curl -sS -m {HOOK_TIMEOUT_SECS} -o /dev/null -X POST \
         -H @\"$HOME/.claude/{HOOK_HEADERS_FILE}\" \
         -H \"X-Fleet-Pane: ${{TMUX_PANE:-}}\" \
         -H 'Content-Type: application/json' \
         --data-binary @- {} || true",
        crate::shell::quote(hook_url)
    )
}

fn command_hook_entry(hook_url: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": session_start_command(hook_url),
        "async": true,
        "timeout": HOOK_TIMEOUT_SECS
    })
}
```

In `strip_fleet`, widen `cmd_match` to also match `c.contains(HOOK_HEADERS_FILE)`. In the merge loop:

```rust
    for (event, matcher, kind) in FLEET_HOOK_EVENTS {
        let mut arr = strip_fleet(hooks.get(*event).unwrap_or(&serde_json::json!([])));
        let entry = match kind {
            HookKind::Http => hook_entry(hook_url, token),
            HookKind::Command => command_hook_entry(hook_url),
        };
        arr.as_array_mut().unwrap().push(serde_json::json!({ "matcher": matcher, "hooks": [entry] }));
        hooks.insert((*event).to_string(), arr);
    }
```

Update any other user of `FLEET_HOOK_EVENTS` (grep) for the 3-tuple. Update the doc comments above it (SessionStart is now installed as a command hook; `clear`/`resume` close the conversation, not the session).

- [ ] **Step 3: Write the headers file**

Local — in `install_hook_at`, after writing settings: write `settings_path.with_file_name(HOOK_HEADERS_FILE)` with `super::provision::write_private_file(&path, &hook_headers_content(token))` when its content differs; count it as `Written`. Test:

```rust
    #[test]
    fn install_writes_a_private_headers_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let hdr = path.with_file_name(HOOK_HEADERS_FILE);
        assert_eq!(std::fs::read_to_string(&hdr).unwrap(), "Authorization: Bearer tok\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&hdr).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
```

Idempotency: the second `install_hook_at` returns `Unchanged` only if both files are unchanged — extend `install_hook_at_creates_then_is_idempotent` accordingly.

Remote — in `provision_hook`, after writing settings.json:

```rust
    write_host_file_secret(
        ssh,
        host,
        CLAUDE_DIR,
        super::hooks_install::HOOK_HEADERS_FILE,
        &super::hooks_install::hook_headers_content(token),
    )
    .await
```

Extend the existing `provision_hook` test (fake `SshExec`) to assert the headers file write.

- [ ] **Step 4: Run** full `cargo test -p fleet-core` → PASS. **Commit:**

```bash
git -C "$WT" commit -am "feat(hooks-install): pane header, SessionStart command hook, compaction and clear/resume events"
```

---

### Task 7: Tasks survive `/clear`; conversation API (Tauri + MCP)

**Files:**
- Modify: `crates/fleet-core/src/service/tasks.rs` (~500–515 and its caller `sweep_open_tasks`)
- Modify: `crates/fleet-core/src/service/transcript.rs` (`TranscriptArgs` override), `crates/fleet-core/src/mcp/tools/messaging.rs` (+ params struct), `crates/fleet-core/src/mcp/tools/orchestration.rs:87` (conversation tool arg)
- Modify: `src-tauri/src/commands/sessions.rs` (~275–300), `src-tauri/src/lib.rs:235`
- Modify: `src/lib/conversation.ts`
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: `Store::current_conversation_source`, `Store::list_conversations`.
- Produces: Tauri `session_conversations({ args: { session_id, limit? } }) -> ConversationRow[]`; MCP `session_conversations`; `session_conversation` args gain `claude_session_id?: string`; TS `listConversations(sessionId)`, `ConversationSummary` type, `Conversation.context`.

- [ ] **Step 1: Failing task test** (`service/tasks.rs` tests, next to the existing "recreated onto a new Claude conversation" test at ~806):

```rust
    #[test]
    fn a_clear_inside_the_worker_does_not_fail_its_task() {
        // Arrange exactly like the existing recreate test, then instead of
        // set_claude_session_id (source fleet) rebind with source Clear.
        // … existing setup producing store `s`, worker `id`, task on "uuid-w" …
        s.rebind_conversation(id, "uuid-x", StartSource::Clear, None, None).unwrap();
        let failed = sweep_open_tasks(&s, now_unix()).unwrap();
        assert!(failed.is_empty());
    }
```

Copy the setup block verbatim from the recreate test in the same module (it creates the worker with `set_claude_session_id(id, "uuid-w")` and dispatches a task). Keep the existing recreate test: `set_claude_session_id` now rebinds with `Fleet`, so it must still fail the task.

- [ ] **Step 2: Implement**: give the pure check an extra parameter `current_source: Option<&str>` and skip the id-mismatch failure when it is one of `clear | resume | compact | startup | unknown`:

```rust
    if let (Some(then), Some(now_id)) = (&task.worker_claude_session_id, &w.claude_session_id) {
        let in_session_switch = matches!(
            current_source,
            Some("clear" | "resume" | "compact" | "startup" | "unknown")
        );
        if then != now_id && !in_session_switch {
            return Some(format!(
                "worker session {wid} was recreated onto a new Claude conversation"
            ));
        }
    }
```

`sweep_open_tasks` passes `s.current_conversation_source(wid).ok().flatten().as_deref()`.

Close the conversation on kill: in `service/sessions/lifecycle.rs` where the `killed` timeline event is written (~860–888), add

```rust
        if let Some(cid) = row.claude_session_id.as_deref() {
            let _ = s.close_conversation(row.id, cid, "killed");
        }
```

(the rows are removed later by `ON DELETE CASCADE` if the session row is deleted). Test in the lifecycle tests: kill a row bound to A → A's conversation has `end_reason = "killed"` (skip if the kill path deletes the row immediately — then assert nothing throws).

Run `cargo test -p fleet-core service::tasks service::sessions` → PASS.

- [ ] **Step 3: Transcript override**

`TranscriptArgs` gets no new field; instead a helper:

```rust
/// [`resolve_args`] for a specific conversation of the row. `E_INVALID` when
/// `claude_session_id` is not one of the row's conversations. The transcript
/// path is that conversation's (falls back to the cwd search).
pub fn resolve_args_for(
    store: &Mutex<Store>,
    row: &SessionRow,
    claude_session_id: &str,
    turns: usize,
    max_chars: usize,
) -> Result<TranscriptArgs, IpcError> {
    crate::validate::claude_session_id(claude_session_id)?;
    let mut args = resolve_args(store, row, turns, max_chars)?;
    if args.claude_session_id == claude_session_id {
        return Ok(args);
    }
    let conv = {
        let s = lock(store)?;
        s.list_conversations(row.id, 500)?
            .into_iter()
            .find(|c| c.claude_session_id == claude_session_id)
    };
    let conv = conv.ok_or_else(|| {
        IpcError::new(codes::E_INVALID, "claude_session_id is not a conversation of this session")
    })?;
    args.claude_session_id = conv.claude_session_id;
    args.transcript_path = conv.transcript_path;
    Ok(args)
}
```

Test: a row with conversations A (current) and B; `resolve_args_for(…, B)` returns B's path; an unknown id → `E_INVALID`.

Also store the hook-validated transcript path on the conversation row: in `remember_transcript_path` (hooks.rs) additionally run
`UPDATE conversations SET transcript_path=?1 WHERE claude_session_id=?2` via a new `Store::set_conversation_transcript_path(claude_session_id, path)`.

- [ ] **Step 4: Tauri commands**

In `src-tauri/src/commands/sessions.rs`, `SessionConversationArgs` gains `pub claude_session_id: Option<String>`; the command uses `resolve_args_for` when present, and only writes the context back (Task 4) when the resolved id equals the row's current id.

New command next to `session_history`:

```rust
#[derive(serde::Deserialize)]
pub struct SessionConversationsArgs {
    pub session_id: i64,
    pub limit: Option<i64>,
}

/// Conversations a session has run, newest first (migration 034). Default
/// 50, max 500.
#[tauri::command]
pub async fn session_conversations(
    args: SessionConversationsArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<fleet_core::store::ConversationRow>, IpcError> {
    let limit = args.limit.unwrap_or(50).clamp(1, 500);
    let s = fleet_core::ipc_error::lock(&store)?;
    s.list_conversations(args.session_id, limit)
}
```

Register in `src-tauri/src/lib.rs` `generate_handler!` next to `session_history`. Match the exact import paths / lock helper the neighbouring commands in this file use.

- [ ] **Step 5: MCP tool** in `mcp/tools/messaging.rs` next to `session_history`:

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SessionConversationsParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Max rows, newest first (default 20).
    pub limit: Option<i64>,
}

    #[tool(
        description = "List the Claude Code conversations a session has run, newest \
        first: claude_session_id, started_at, ended_at, start_source (startup, resume, \
        clear, compact, fork, fleet, unknown), end_reason, model, first_prompt, turns, \
        compactions and current. Pass a claude_session_id to session_conversation to \
        read an earlier one."
    )]
    pub(super) async fn session_conversations(
        &self,
        Parameters(p): Parameters<SessionConversationsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_conversations", &format!("session_id={}", p.session_id));
        let limit = p.limit.unwrap_or(20).clamp(1, 500);
        let rows = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.list_conversations(p.session_id, limit).map_err(to_mcp_err)?
        };
        ok_json(&rows)
    }
```

Place the params struct where the other `*Params` of this module live and match their derive list exactly. Add the tool to the `generate_handler!` / router list. Add the optional `claude_session_id` param to the MCP `session_conversation` tool in `orchestration.rs:87` the same way as the Tauri command.

Also tighten the `session_history` description: mention conversation events (`conversation_started`, `conversation_ended`, `compact_started`, `compact_done`, `turn_done`).

- [ ] **Step 6: Regenerate the reference**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
Then: `cargo test -p fleet-core reference_is_current` → PASS.

- [ ] **Step 7: TS**

`src/lib/conversation.ts`:

```ts
export interface ConversationSummary {
  id: number;
  session_id: number;
  claude_session_id: string;
  transcript_path: string | null;
  started_at: number;
  ended_at: number | null;
  start_source: 'startup' | 'resume' | 'clear' | 'compact' | 'fork' | 'fleet' | 'unknown';
  end_reason: string | null;
  model: string | null;
  first_prompt: string | null;
  turns: number;
  compactions: number;
  current: boolean;
}

export interface ContextView {
  tokens: number;
  window: number;
  pct: number;
  stale: boolean;
}

export function listConversations(sessionId: number, limit = 50) {
  return invokeCmd<ConversationSummary[]>('session_conversations', {
    args: { session_id: sessionId, limit },
  });
}
```

Add `context: ContextView | null` to `Conversation`, and an optional `claudeSessionId?: string` parameter to `sessionConversation` passed as `claude_session_id`. Match the `invokeCmd` generic/usage of the neighbouring functions. Add a Vitest case in the existing `conversation.test.ts` (or create it) asserting `listConversations` calls `invokeCmd` with `session_conversations` and the args shape (mock the same way sibling tests mock `invokeCmd`).

- [ ] **Step 8: Run everything**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npx svelte-check
npx vitest run
```

All unpiped, all PASS.

- [ ] **Step 9: Commit**

```bash
git -C "$WT" add -A
git -C "$WT" commit -m "feat(api): session_conversations, read earlier conversations; tasks survive /clear"
```

---

### Task 8: End-to-end verification and docs

**Files:**
- Modify: `docs/superpowers/specs/2026-09-18-conversation-events-design.md` (§1.1: there is no existing Settings "hooks outdated" status — replace that sentence with "Hosts pick up the new entries on the next `provision_hosts`; the local host on app start.")
- Modify: `docs/control-api.md` (one paragraph: conversations, `session_conversations`, the SessionStart command hook and `fleet-hook.headers`)
- Modify: `CLAUDE.md` *Status & known issues* (one line: conversation tracking, migration 034)

- [ ] **Step 1: Local CI**

Run: `scripts/ci-local.sh` (unpiped). Expected: every stage green. If the cargo target volume is unmounted, set `CARGO_TARGET_DIR` to the scratchpad per the repo memory.

- [ ] **Step 2: Manual check on the local host** (dev build, MCP server enabled so the hook auto-installs)

1. `jq '.hooks | keys' ~/.claude/settings.json` lists the 9 events; `stat -f %Lp ~/.claude/fleet-hook.headers` prints `600`.
2. Start a fleet session, send two prompts. `session_conversations` → one row, `turns = 2`, `start_source = fleet`.
3. `/clear` in the pane. Within ~1 s: `list_sessions` shows the new `claude_session_id`, `context_tokens = 0`, `context_pct = 0`; `session_conversations` → 2 rows, old `end_reason = clear`; `session_history` has `conversation_ended` then `conversation_started(clear)`.
4. Prompt once: `context_source = transcript`, tokens > 0.
5. `/compact`: `compact_started`, then one `compact_done`; `context_stale = true` until the next reply.
6. `/resume` to the first conversation: still 2 rows, the first is current again.
7. Two sessions in one cwd: repeat step 3 in one of them; only that row rebinds.

Record the outcome of each step in the PR description.

- [ ] **Step 3: Commit docs**

```bash
git -C "$WT" commit -am "docs: conversation tracking in control-api, CLAUDE.md status, spec correction"
```

---

## Self-Review Notes

- Spec §1.1 → Task 6; §1.2 → Task 3; §1.3 → Tasks 1–3; §1.4 → Task 5 (+ `set_claude_session_id` in Task 2); §1.5 → Tasks 4–5; §1.6 → Task 2 (push) + Task 3 (`turn_done`); §1.7 → Task 7; §1.8 → Tasks 1, 7. Edge cases: `/clear` mid-turn (Task 3 test), resume back (Task 2 test), compaction dedupe (Tasks 2–3), master caller (Task 3), stale awaiting mark (TTL in `sessions_awaiting_rebind`), transcript deleted (existing `E_NO_TRANSCRIPT`), kill → Task 7 Step 2.
- Empty conversations (`turns = 0`, not current) are kept in the store; hiding them is a Phase 2 UI rule.
