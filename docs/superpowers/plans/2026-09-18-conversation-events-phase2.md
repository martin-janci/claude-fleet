# Conversation Event Tracking — Phase 2 (Conversations UI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Conversations tab shows which Claude conversation you are looking at, lets you switch to earlier ones, follows `/clear`/`/resume` automatically, shows context/model/status in a header, and renders compactions, slash commands, interrupts, failures and permission waits inline.

**Architecture:** Backend: `parse_conversation` learns three new item kinds (compact, command, interrupt) and `session_conversation` returns the conversation's timeline events. Frontend: a tiny `live_events.ts` registry fans the already-emitted `session:event` / `session:conversations` pushes out to components; pure helpers in `conversation.ts` build the header model and merge turns with events; a new `ConversationHeader.svelte` renders the header and switcher; `ConversationPanel.svelte` resets on conversation change, supports read-only viewing of an earlier conversation, and renders the new rows. `Timeline.svelte` switches from field-keyed refetch to the push.

**Tech Stack:** Rust (fleet-core, serde), Svelte 5 runes, TypeScript, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-18-conversation-events-design.md` — Phase 2 (§2.1–§2.4), Edge cases, Testing. Phase 1 (PR #130) is the base: `ConversationSummary`, `listConversations`, `session_conversation { claude_session_id }`, `SessionRow.{model, context_tokens, context_window, context_stale}`, `SessionEvent.claude_session_id`, and the `session:event` / `session:conversations` bus events already exist.

## Global Constraints

- Branch `feature/conversation-ui-phase2` (based on phase 1's `feature/conversation-event-tracking-df4955`). All git via `git -C <worktree>`; no pull/push/rebase/checkout/stash by subagents.
- Wire fields snake_case; Rust `Option<T>` ↔ TS `T | null`. New Rust `ConvItem` variants serialize with `kind` in snake_case, like the existing ones.
- Frontend commands: `npx vitest run`, `npx svelte-check`. Rust: full `cargo test -p fleet-core` unpiped before each commit; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`.
- Never hold the `Mutex<Store>` guard across `.await`.
- Keep existing `data-testid`s working; the context meter keeps `data-testid="conv-ctx"` (it moves into the header).
- Transcript text is never logged.
- No new npm dependencies.
- Colours: reuse the CSS vars the panel already uses (`--bg`, `--bg-pane`, `--border`, `--fg`, `--fg-muted`, `--accent`, `--mono`, `--usage-warn`, `--usage-crit`); amber `#e6a23c` / red `#e64a4a` only where the panel already hardcodes them.

## File Map

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/transcript.rs` | New `ConvItem` variants + parser rules; `Conversation.events`; plain-text rendering of new items |
| `crates/fleet-core/src/store/timeline.rs` | `list_conversation_events(session_id, claude_session_id, limit)` |
| `src/lib/conversation.ts` | TS mirrors; header/meter/switcher/thread helpers |
| `src/lib/live_events.ts` (new) | Per-session registry for pushed timeline events and conversation-list changes |
| `src/lib/events.ts`, `src/App.svelte` | Subscribe to `session:event` / `session:conversations` and feed `live_events` |
| `src/lib/ConversationHeader.svelte` (new) | Header bar: switcher, context meter, model, status, last event |
| `src/lib/ConversationPanel.svelte` | Mount header; conversation reset; read-only earlier view; switch notice; render new rows |
| `src/lib/Timeline.svelte` | Prepend pushed events |
| Tests | `conversation.test.ts`, `live_events.test.ts` (new), `ConversationHeader.test.ts` (new), `ConversationPanel.test.ts`, `Timeline.test.ts` (new), Rust tests in `transcript.rs` / `timeline.rs` |

Real transcript shapes (sampled from local `~/.claude/projects/**.jsonl`, Claude Code 2.1.2xx) used by the parser tests:

```json
{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":1013334},"timestamp":"…"}
{"type":"user","isCompactSummary":true,"isVisibleInTranscriptOnly":true,"message":{"content":"This session is being continued from a previous conversation…"}}
{"type":"user","message":{"content":"<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args>opus</command-args>"}}
{"type":"user","message":{"content":"<local-command-stdout>Set model to opus</local-command-stdout>"}}
{"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"Base directory for this skill: …"}]}}
{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}
{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}
```

---

### Task 1: Backend — parser items and conversation events

**Files:**
- Modify: `crates/fleet-core/src/service/transcript.rs` (`ConvItem`, `Conversation`, `parse_conversation`, `parse_turns`/render, `trim_conversation`, `fetch_conversation_for_row`)
- Modify: `crates/fleet-core/src/store/timeline.rs`
- Modify: `src/lib/conversation.ts` (TS mirrors only)

**Interfaces:**
- Produces (Rust):
  - `ConvItem::Compact { trigger: Option<String>, pre_tokens: Option<i64>, summary: Option<String> }`
  - `ConvItem::Command { name: String, args: Option<String>, output: Option<String> }`
  - `ConvItem::Interrupt { during_tool: bool }`
  - `Conversation.events: Vec<SessionEvent>` (oldest first; this conversation only)
  - `Store::list_conversation_events(&self, session_id: i64, claude_session_id: &str, limit: i64) -> Result<Vec<SessionEvent>, IpcError>` (oldest first)
- Produces (TS, `conversation.ts`):
  ```ts
  export type ConvItem =
    | { kind: 'text'; text: string }
    | { kind: 'tool'; summary: string; error?: boolean }
    | { kind: 'compact'; trigger: string | null; pre_tokens: number | null; summary: string | null }
    | { kind: 'command'; name: string; args: string | null; output: string | null }
    | { kind: 'interrupt'; during_tool: boolean };
  // Conversation gains:
  events: SessionEvent[];   // import type { SessionEvent } from './timeline'
  ```

- [ ] **Step 1: Failing parser tests** (in `transcript.rs` tests):

```rust
    fn jl(lines: &[serde_json::Value]) -> String {
        lines.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n")
    }
    fn user(content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"user","timestamp":"2026-09-18T10:00:00Z","message":{"content":content}})
    }
    fn asst(text: &str) -> serde_json::Value {
        serde_json::json!({"type":"assistant","timestamp":"2026-09-18T10:00:05Z",
            "message":{"content":[{"type":"text","text":text}]}})
    }

    #[test]
    fn a_compaction_is_its_own_item_with_its_summary_not_a_prompt() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("first")),
            asst("done"),
            serde_json::json!({"type":"system","subtype":"compact_boundary","timestamp":"2026-09-18T10:01:00Z",
                "compactMetadata":{"trigger":"auto","preTokens":180000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"timestamp":"2026-09-18T10:01:00Z",
                "message":{"content":"This session is being continued… Summary: A"}}),
            user(serde_json::json!("next")),
        ]));
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].prompt, None);
        assert_eq!(
            t[1].items,
            vec![ConvItem::Compact {
                trigger: Some("auto".into()),
                pre_tokens: Some(180_000),
                summary: Some("This session is being continued… Summary: A".into()),
            }]
        );
        assert_eq!(t[2].prompt.as_deref(), Some("next"));
    }

    #[test]
    fn a_slash_command_opens_a_turn_and_collects_its_output() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("<command-name>/model</command-name>\n  <command-message>model</command-message>\n  <command-args>opus</command-args>")),
            user(serde_json::json!("<local-command-stdout>Set model to opus</local-command-stdout>")),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt, None);
        assert_eq!(
            t[0].items,
            vec![ConvItem::Command {
                name: "/model".into(),
                args: Some("opus".into()),
                output: Some("Set model to opus".into()),
            }]
        );
    }

    #[test]
    fn a_command_without_args_has_none() {
        let t = parse_conversation(&jl(&[user(serde_json::json!(
            "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"
        ))]));
        assert_eq!(t[0].items, vec![ConvItem::Command { name: "/clear".into(), args: None, output: None }]);
    }

    #[test]
    fn meta_entries_are_skipped() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("real prompt")),
            serde_json::json!({"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"Base directory for this skill: x"}]}}),
            asst("ok"),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt.as_deref(), Some("real prompt"));
        assert_eq!(t[0].items, vec![ConvItem::Text { text: "ok".into() }]);
    }

    #[test]
    fn an_interrupt_is_an_item_of_the_current_turn() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            asst("working"),
            user(serde_json::json!([{"type":"text","text":"[Request interrupted by user for tool use]"}])),
            user(serde_json::json!([{"type":"text","text":"[Request interrupted by user]"}])),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(
            t[0].items,
            vec![
                ConvItem::Text { text: "working".into() },
                ConvItem::Interrupt { during_tool: true },
                ConvItem::Interrupt { during_tool: false },
            ]
        );
    }

    #[test]
    fn plain_text_rendering_names_the_new_items() {
        let turns = parse_turns(&jl(&[
            user(serde_json::json!("<command-name>/model</command-name><command-args>opus</command-args>")),
            asst("switched"),
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"manual","preTokens":5}}),
        ]));
        let all = turns.join("\n");
        assert!(all.contains("[command] /model opus"));
        assert!(all.contains("[compacted] manual"));
    }
```

(`parse_turns` currently drops turns with no items and joins items; make it render `[command] <name> <args>`, `[compacted] <trigger>`, `[interrupted]` lines. If `parse_turns` drops a turn whose only item is `Compact`, change it to keep turns with any item.)

- [ ] **Step 2: Run to fail** — `cargo test -p fleet-core service::transcript` → compile errors.

- [ ] **Step 3: Implement the variants and parser rules**

`ConvItem` additions (keep the serde attributes of the enum):

```rust
    /// A compaction (`system/compact_boundary`). `summary` is the text of the
    /// following `isCompactSummary` user entry when the read tail has it.
    Compact {
        trigger: Option<String>,
        pre_tokens: Option<i64>,
        summary: Option<String>,
    },
    /// A slash command the user ran (`<command-name>` user entry); `output`
    /// is the following `<local-command-stdout>` / `<local-command-stderr>`.
    Command {
        name: String,
        args: Option<String>,
        output: Option<String>,
    },
    /// `[Request interrupted by user]` (`during_tool`: "… for tool use").
    Interrupt { during_tool: bool },
```

Caps (constants next to the others): `COMPACT_SUMMARY_MAX_CHARS: usize = 20_000`, `COMMAND_OUTPUT_MAX_CHARS: usize = 4_000`. Truncate by chars and append `…` when cut.

Helpers:

```rust
/// Text between `<tag>` and `</tag>`, trimmed; `None` when absent or empty.
fn tag_text(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    let t = s[start..end].trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The plain text of a user entry's content (string body, or its text
/// blocks joined), without deciding whether it is a prompt.
fn user_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            {
                return None;
            }
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
```

Parser changes (in `parse_conversation`, keep all existing behaviour):

```rust
            "system" if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") => {
                push(&mut turns, current.take());
                tool_items.clear();
                let meta = v.get("compactMetadata");
                current = Some(ConvTurn {
                    prompt: None,
                    at: at(),
                    ended_at: None,
                    items: vec![ConvItem::Compact {
                        trigger: meta.and_then(|m| m.get("trigger")).and_then(|t| t.as_str()).map(String::from),
                        pre_tokens: meta.and_then(|m| m.get("preTokens")).and_then(|t| t.as_i64()),
                        summary: None,
                    }],
                });
            }
            "user" => {
                // (existing tool_result error flagging stays first)
                if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
                    continue;
                }
                if v.get("isCompactSummary").and_then(|b| b.as_bool()) == Some(true) {
                    if let (Some(text), Some(turn)) = (user_text(content), current.as_mut()) {
                        if let Some(ConvItem::Compact { summary, .. }) = turn.items.last_mut() {
                            if summary.is_none() {
                                *summary = Some(cap_chars(text.trim(), COMPACT_SUMMARY_MAX_CHARS));
                            }
                        }
                    }
                    continue;
                }
                if let Some(text) = user_text(content) {
                    if let Some(name) = tag_text(&text, "command-name") {
                        push(&mut turns, current.take());
                        tool_items.clear();
                        current = Some(ConvTurn {
                            prompt: None,
                            at: at(),
                            ended_at: None,
                            items: vec![ConvItem::Command { name, args: tag_text(&text, "command-args"), output: None }],
                        });
                        continue;
                    }
                    if let Some(out) = tag_text(&text, "local-command-stdout")
                        .or_else(|| tag_text(&text, "local-command-stderr"))
                    {
                        if let Some(ConvItem::Command { output, .. }) =
                            current.as_mut().and_then(|t| t.items.iter_mut().rev().find(|i| matches!(i, ConvItem::Command { .. })))
                        {
                            if output.is_none() {
                                *output = Some(cap_chars(&out, COMMAND_OUTPUT_MAX_CHARS));
                            }
                        }
                        continue;
                    }
                    if text.trim_start().starts_with("[Request interrupted by user") {
                        let turn = current.get_or_insert_with(|| ConvTurn { prompt: None, at: at(), ended_at: None, items: Vec::new() });
                        turn.items.push(ConvItem::Interrupt { during_tool: text.contains("for tool use") });
                        continue;
                    }
                }
                // existing: if let Some(prompt) = prompt_text(content) { … open a prompt turn … }
            }
```

A `<local-command-stdout>` with no preceding command in the current turn is dropped (never becomes a prompt). Adapt `trim_conversation`'s char accounting: count `summary`/`output`/`name`+`args` lengths for the new variants (the char budget must still bound the payload).

- [ ] **Step 4: Conversation events**

`store/timeline.rs`:

```rust
    /// This conversation's timeline events, oldest first (at most `limit`,
    /// the newest ones). Events recorded before migration 034 carry no
    /// conversation id and are not returned.
    pub fn list_conversation_events(
        &self,
        session_id: i64,
        claude_session_id: &str,
        limit: i64,
    ) -> Result<Vec<SessionEvent>, crate::ipc_error::IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, at, kind, detail, claude_session_id FROM (\
                 SELECT * FROM session_events WHERE session_id = ?1 AND claude_session_id = ?2 \
                 ORDER BY at DESC, id DESC LIMIT ?3) ORDER BY at ASC, id ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, claude_session_id, limit], |row| {
            Ok(SessionEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                at: row.get(2)?,
                kind: row.get(3)?,
                detail: row.get(4)?,
                claude_session_id: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
```

Test: two conversations' events + one NULL-id event → only the requested conversation's, oldest first, `limit` keeps the newest.

`Conversation` gains `pub events: Vec<SessionEvent>` (empty wherever a `Conversation` is built without a store). `fetch_conversation_for_row` fills it after the fetch: `s.list_conversation_events(row.id, &claude_id, CONV_EVENTS_LIMIT)` with `const CONV_EVENTS_LIMIT: i64 = 200;`, best-effort (`unwrap_or_default()`), store lock taken briefly after the await. Test: `fetch_conversation_for_row` on a temp transcript (reuse the Task 4/7 test harness in this file) returns the events of the requested conversation only.

- [ ] **Step 5: TS mirrors** in `src/lib/conversation.ts`: the `ConvItem` union above; `Conversation.events: SessionEvent[]`; add `events: []` to every fixture that builds a `Conversation` (`ConversationPanel.test.ts`, `conversation.test.ts`). Extend `ConvGroup` / `groupItems` now so the new kinds type-check and render nowhere yet:

```ts
export type ConvGroup =
  | { kind: 'text'; text: string }
  | { kind: 'tools'; tools: ToolLine[] }
  | { kind: 'compact'; trigger: string | null; pre_tokens: number | null; summary: string | null }
  | { kind: 'command'; name: string; args: string | null; output: string | null }
  | { kind: 'interrupt'; during_tool: boolean };
```

In `groupItems`, `tool` items keep folding into the open `tools` group; `text` stays as is; each `compact` / `command` / `interrupt` item closes any open tool run and becomes its own group with the same fields (drop `kind: 'tool'`'s grouping only). Add the `groupItems` test from Task 2 Step 3 ("new item kinds are their own groups and break a tool run") here. `ConversationPanel.svelte`'s `{#each groups}` must ignore the three new group kinds until Task 4 (an `{:else}` branch rendering nothing) so svelte-check passes.

- [ ] **Step 6: Run** — full `cargo test -p fleet-core`, clippy, fmt, `cargo check -p claude-fleet`, `npx svelte-check`, `npx vitest run` → PASS. `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` only if a tool description changed (it should not).

- [ ] **Step 7: Commit**

```bash
git -C "$WT" commit -am "feat(transcript): compaction, slash-command and interrupt items; conversation events in session_conversation"
```

---

### Task 2: Frontend data layer — live events and thread helpers

**Files:**
- Create: `src/lib/live_events.ts`, `src/lib/live_events.test.ts`
- Modify: `src/lib/events.ts` (two batched handlers + two `sub(...)` registrations)
- Modify: `src/App.svelte` (pass the handlers)
- Modify: `src/lib/conversation.ts`, `src/lib/conversation.test.ts`

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  ```ts
  // live_events.ts
  export function onTimelineEvent(sessionId: number, fn: (e: SessionEvent) => void): () => void;
  export function onConversationsChanged(sessionId: number, fn: () => void): () => void;
  export function dispatchTimelineEvents(events: SessionEvent[]): void;
  export function dispatchConversationsChanged(sessionIds: number[]): void;
  // events.ts RowEventHandlers gains
  onTimelineEvents?: (events: SessionEvent[]) => void;
  onConversationsChanged?: (sessionIds: number[]) => void;
  // conversation.ts
  export function formatTokens(n: number): string;
  export interface ContextMeter { pct: number; level: ContextLevel; label: string; title: string; stale: boolean }
  export function contextMeter(s: Pick<SessionRow, 'context_pct' | 'context_tokens' | 'context_window' | 'context_stale'>): ContextMeter | null;
  export const SOURCE_LABELS: Record<ConversationSummary['start_source'], string>;
  export function switcherEntries(list: ConversationSummary[]): ConversationSummary[];
  export function conversationTitle(c: ConversationSummary): string;
  export function statusChip(s: Pick<SessionRow, 'claude_status' | 'current_activity'>): string | null;
  export interface InlineEvent { id: number; at: number; label: string; detail: string | null; tone: 'info' | 'warn' | 'error' }
  export function inlineEventFor(e: SessionEvent, live: { latestId: number | null; blocked: boolean }): InlineEvent | null;
  export type ThreadRow = { kind: 'turn'; turn: ConvTurn; index: number } | { kind: 'event'; event: InlineEvent };
  export function buildThread(turns: ConvTurn[], events: SessionEvent[], live: { blocked: boolean }, truncated: boolean): ThreadRow[];
  export function lastEventLabel(events: SessionEvent[], nowMs: number): string | null;
  export function mergeEvents(base: SessionEvent[], extra: SessionEvent[]): SessionEvent[];
  ```

- [ ] **Step 1: `live_events.ts` with tests**

```ts
// live_events.ts
/**
 * Per-session fan-out of the backend's timeline pushes (`session:event`) and
 * conversation-list changes (`session:conversations`). App.svelte feeds it
 * from its single `subscribeToRowEvents`; components register for one
 * session and get only that session's events.
 */
import type { SessionEvent } from './timeline';

type Subs<T> = Map<number, Set<(v: T) => void>>;
const timelineSubs: Subs<SessionEvent> = new Map();
const conversationSubs: Subs<void> = new Map();

function add<T>(subs: Subs<T>, id: number, fn: (v: T) => void): () => void {
  let set = subs.get(id);
  if (!set) {
    set = new Set();
    subs.set(id, set);
  }
  set.add(fn);
  return () => {
    set!.delete(fn);
    if (set!.size === 0) subs.delete(id);
  };
}

export function onTimelineEvent(sessionId: number, fn: (e: SessionEvent) => void): () => void {
  return add(timelineSubs, sessionId, fn);
}

export function onConversationsChanged(sessionId: number, fn: () => void): () => void {
  return add(conversationSubs, sessionId, () => fn());
}

export function dispatchTimelineEvents(events: SessionEvent[]): void {
  for (const e of events) {
    for (const fn of timelineSubs.get(e.session_id) ?? []) fn(e);
  }
}

export function dispatchConversationsChanged(sessionIds: number[]): void {
  for (const id of new Set(sessionIds)) {
    for (const fn of conversationSubs.get(id) ?? []) fn();
  }
}
```

```ts
// live_events.test.ts
import { describe, it, expect, vi } from 'vitest';
import { onTimelineEvent, onConversationsChanged, dispatchTimelineEvents, dispatchConversationsChanged } from './live_events';

const ev = (id: number, session_id: number) =>
  ({ id, session_id, at: 1, kind: 'compact_done', detail: null, claude_session_id: 'a' });

describe('live_events', () => {
  it('delivers a timeline event only to its session and stops after unsubscribe', () => {
    const a = vi.fn(), b = vi.fn();
    const offA = onTimelineEvent(1, a);
    onTimelineEvent(2, b);
    dispatchTimelineEvents([ev(10, 1)]);
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).not.toHaveBeenCalled();
    offA();
    dispatchTimelineEvents([ev(11, 1)]);
    expect(a).toHaveBeenCalledTimes(1);
  });

  it('coalesces duplicate conversation-list changes per flush', () => {
    const f = vi.fn();
    onConversationsChanged(3, f);
    dispatchConversationsChanged([3, 3, 4]);
    expect(f).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Wire `events.ts` and `App.svelte`**

In `RowEventHandlers` (batched section) add:

```ts
  /** One call per flush with every `session:event` (timeline push). */
  onTimelineEvents?: (events: TimelineEvent[]) => void;
  /** One call per flush with the ids from every `session:conversations`. */
  onConversationsChanged?: (sessionIds: number[]) => void;
```

In the flush, replace the inert `case 'session:event': case 'session:conversations': break;` with collection into two arrays and call the handlers once per flush (same pattern as the other batched kinds). Register `sub('session:event', !!handlers.onTimelineEvents)` and `sub('session:conversations', !!handlers.onConversationsChanged)` in the `Promise.all` list (use the existing `wanted` computation style). In `App.svelte`'s `subscribeToRowEvents({...})` add `onTimelineEvents: dispatchTimelineEvents, onConversationsChanged: dispatchConversationsChanged` (import from `./lib/live_events`). Add a unit test in an existing or new `events.test.ts` only if the file's `listen` can be mocked the way other tests mock `@tauri-apps/api/event`; otherwise rely on `live_events.test.ts` and the panel tests.

- [ ] **Step 3: Helpers in `conversation.ts` + tests**

```ts
import { contextLevel, type ContextLevel } from './attention';
import type { SessionEvent } from './timeline';

export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
  const m = n / 1_000_000;
  return `${Number.isInteger(m) ? m : m.toFixed(2).replace(/0+$/, '').replace(/\.$/, '')}M`;
}

export interface ContextMeter {
  pct: number;
  level: ContextLevel;
  label: string;
  title: string;
  stale: boolean;
}

export function contextMeter(
  s: Pick<SessionRow, 'context_pct' | 'context_tokens' | 'context_window' | 'context_stale'>,
): ContextMeter | null {
  const pct =
    s.context_pct ??
    (s.context_tokens != null && s.context_window ? (s.context_tokens * 100) / s.context_window : null);
  const level = contextLevel(pct);
  if (pct === null || level === null) return null;
  const rounded = Math.round(pct);
  const label =
    s.context_tokens != null && s.context_window
      ? `${formatTokens(s.context_tokens)} / ${formatTokens(s.context_window)} · ${rounded}%`
      : `ctx ${rounded}%`;
  const title = s.context_stale
    ? 'Context size from before the last compaction or resume — it updates with the next reply'
    : `Context window ${rounded}% used`;
  return { pct, level, label, title, stale: !!s.context_stale };
}

export const SOURCE_LABELS: Record<ConversationSummary['start_source'], string> = {
  startup: 'started',
  resume: '/resume',
  clear: '/clear',
  compact: '/compact',
  fork: 'fork',
  fleet: 'started by fleet',
  unknown: 'new conversation',
};

/** Switcher rows: drop empty non-current conversations (a `/clear` right
 *  after a `/clear`), keep order (newest first). */
export function switcherEntries(list: ConversationSummary[]): ConversationSummary[] {
  return list.filter((c) => c.current || c.turns > 0 || c.first_prompt !== null);
}

function clock(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

export function conversationTitle(c: ConversationSummary): string {
  const when = c.current ? 'Current' : clock(c.started_at);
  const turns = `${c.turns} turn${c.turns === 1 ? '' : 's'}`;
  return `${when} · ${SOURCE_LABELS[c.start_source]} · ${turns}`;
}

export function statusChip(s: Pick<SessionRow, 'claude_status' | 'current_activity'>): string | null {
  if (s.current_activity === 'compacting') return 'compacting';
  return s.claude_status ?? null;
}

export interface InlineEvent {
  id: number;
  at: number;
  label: string;
  detail: string | null;
  tone: 'info' | 'warn' | 'error';
}

const PERMISSION_KINDS = new Set(['permission_prompt', 'elicitation_dialog', 'elicitation_url_dialog']);

function humanError(detail: string | null): { head: string; rest: string | null } {
  if (!detail) return { head: 'unknown error', rest: null };
  const i = detail.indexOf(':');
  const head = (i < 0 ? detail : detail.slice(0, i)).replace(/_/g, ' ').trim();
  const rest = i < 0 ? null : detail.slice(i + 1).trim() || null;
  return { head, rest };
}

/** The inline row for a timeline event, or null for events the thread does
 *  not show (turn_done, status changes, prompts and compactions — the
 *  transcript already carries those). */
export function inlineEventFor(
  e: SessionEvent,
  live: { latestId: number | null; blocked: boolean },
): InlineEvent | null {
  switch (e.kind) {
    case 'conversation_started':
      return e.detail === 'resume'
        ? { id: e.id, at: e.at, label: 'Resumed conversation', detail: null, tone: 'info' }
        : null;
    case 'stop_failure': {
      const { head, rest } = humanError(e.detail);
      return { id: e.id, at: e.at, label: `Turn failed: ${head}`, detail: rest, tone: 'error' };
    }
    case 'notification': {
      if (!e.detail || !PERMISSION_KINDS.has(e.detail)) return null;
      const waiting = live.blocked && live.latestId === e.id;
      return {
        id: e.id,
        at: e.at,
        label: waiting ? 'Waiting for permission' : 'Asked for permission',
        detail: null,
        tone: waiting ? 'warn' : 'info',
      };
    }
    case 'conversation_ended':
      return { id: e.id, at: e.at, label: `Conversation ended (${e.detail ?? 'unknown'})`, detail: null, tone: 'info' };
    default:
      return null;
  }
}

export type ThreadRow =
  | { kind: 'turn'; turn: ConvTurn; index: number }
  | { kind: 'event'; event: InlineEvent };

/** Interleave turns (ISO `at`) and inline events (unix secs) by time. An
 *  event goes after the last turn that started at or before it. When the
 *  tail is truncated, events older than the first loaded turn are dropped
 *  (their turns are not on screen). */
export function buildThread(
  turns: ConvTurn[],
  events: SessionEvent[],
  live: { blocked: boolean },
  truncated: boolean,
): ThreadRow[] {
  const latestId = events.length ? events[events.length - 1].id : null;
  const inline = events
    .map((e) => inlineEventFor(e, { latestId, blocked: live.blocked }))
    .filter((e): e is InlineEvent => e !== null)
    .sort((a, b) => a.at - b.at || a.id - b.id);
  const starts: number[] = [];
  let prev = -Infinity;
  for (const t of turns) {
    const ms = t.at ? Date.parse(t.at) : NaN;
    prev = Number.isNaN(ms) ? prev : ms / 1000;
    starts.push(prev);
  }
  const rows: ThreadRow[] = [];
  let k = 0;
  const firstStart = starts.length ? starts[0] : Infinity;
  while (k < inline.length && inline[k].at < firstStart) {
    if (!truncated) rows.push({ kind: 'event', event: inline[k] });
    k++;
  }
  turns.forEach((turn, i) => {
    rows.push({ kind: 'turn', turn, index: i });
    const next = i + 1 < starts.length ? starts[i + 1] : Infinity;
    while (k < inline.length && inline[k].at < next) {
      rows.push({ kind: 'event', event: inline[k] });
      k++;
    }
  });
  return rows;
}

const LAST_EVENT_KINDS: Record<string, (d: string | null) => string | null> = {
  compact_done: () => '/compact',
  compact_started: () => 'compacting',
  stop_failure: (d) => humanError(d).head,
  notification: (d) => (d && PERMISSION_KINDS.has(d) ? 'permission asked' : null),
  conversation_started: (d) => (d === 'resume' ? '/resume' : d === 'clear' ? '/clear' : null),
};

/** "`/compact` 3m ago" for the newest notable event, else null. */
export function lastEventLabel(events: SessionEvent[], nowMs: number): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const f = LAST_EVENT_KINDS[events[i].kind];
    const label = f ? f(events[i].detail) : null;
    if (label) return `${label} ${relativeFromUnix(events[i].at, nowMs)}`;
  }
  return null;
}

/** Union by id, oldest first (the panel appends pushed events to the ones
 *  the fetch returned). */
export function mergeEvents(base: SessionEvent[], extra: SessionEvent[]): SessionEvent[] {
  const byId = new Map<number, SessionEvent>();
  for (const e of [...base, ...extra]) byId.set(e.id, e);
  return [...byId.values()].sort((a, b) => a.at - b.at || a.id - b.id);
}
```

`relativeFromUnix` = the existing `timeAgo(unixSecs, nowMs)` from `./session_status` (import it; do not add a second formatter).

(`ConvGroup` / `groupItems` were extended in Task 1 Step 5.)

Tests to add in `conversation.test.ts` (exact expectations):

```ts
describe('formatTokens', () => {
  it('formats', () => {
    expect(formatTokens(950)).toBe('950');
    expect(formatTokens(42_300)).toBe('42k');
    expect(formatTokens(200_000)).toBe('200k');
    expect(formatTokens(1_000_000)).toBe('1M');
    expect(formatTokens(1_250_000)).toBe('1.25M');
  });
});

describe('contextMeter', () => {
  const s = (o: Partial<SessionRow>) =>
    ({ context_pct: null, context_tokens: null, context_window: null, context_stale: false, ...o }) as SessionRow;
  it('shows tokens and window when known', () => {
    expect(contextMeter(s({ context_pct: 21, context_tokens: 42_000, context_window: 200_000 }))?.label)
      .toBe('42k / 200k · 21%');
  });
  it('falls back to the percentage', () => {
    expect(contextMeter(s({ context_pct: 55 }))?.label).toBe('ctx 55%');
  });
  it('is 0 after a clear, not null', () => {
    expect(contextMeter(s({ context_pct: 0, context_tokens: 0, context_window: 200_000 }))?.label)
      .toBe('0 / 200k · 0%');
  });
  it('marks a stale value', () => {
    const m = contextMeter(s({ context_pct: 80, context_stale: true }))!;
    expect(m.stale).toBe(true);
    expect(m.title).toMatch(/before the last compaction/);
  });
  it('is null when nothing is known', () => {
    expect(contextMeter(s({}))).toBeNull();
  });
});

describe('switcherEntries / conversationTitle', () => {
  const c = (o: Partial<ConversationSummary>): ConversationSummary => ({
    id: 1, session_id: 1, claude_session_id: 'a', transcript_path: null, started_at: 1_789_000_000,
    ended_at: null, start_source: 'clear', end_reason: null, model: null, first_prompt: null,
    turns: 0, compactions: 0, current: false, ...o,
  });
  it('hides empty earlier conversations but keeps the current one', () => {
    const list = [c({ id: 3, current: true }), c({ id: 2 }), c({ id: 1, turns: 4 })];
    expect(switcherEntries(list).map((x) => x.id)).toEqual([3, 1]);
  });
  it('titles', () => {
    expect(conversationTitle(c({ current: true, turns: 1 }))).toBe('Current · /clear · 1 turn');
  });
});

describe('inlineEventFor', () => {
  const e = (kind: string, detail: string | null, id = 1) =>
    ({ id, session_id: 1, at: 100, kind, detail, claude_session_id: 'a' });
  it('maps failures, permissions, resume and end; hides the rest', () => {
    expect(inlineEventFor(e('stop_failure', 'rate_limit: slow down'), { latestId: 1, blocked: false }))
      .toMatchObject({ label: 'Turn failed: rate limit', detail: 'slow down', tone: 'error' });
    expect(inlineEventFor(e('notification', 'permission_prompt'), { latestId: 1, blocked: true }))
      .toMatchObject({ label: 'Waiting for permission', tone: 'warn' });
    expect(inlineEventFor(e('notification', 'permission_prompt'), { latestId: 2, blocked: true }))
      .toMatchObject({ label: 'Asked for permission', tone: 'info' });
    expect(inlineEventFor(e('conversation_started', 'resume'), { latestId: 1, blocked: false })?.label)
      .toBe('Resumed conversation');
    expect(inlineEventFor(e('conversation_started', 'clear'), { latestId: 1, blocked: false })).toBeNull();
    expect(inlineEventFor(e('turn_done', 'x'), { latestId: 1, blocked: false })).toBeNull();
    expect(inlineEventFor(e('compact_done', 'auto'), { latestId: 1, blocked: false })).toBeNull();
  });
});

describe('buildThread', () => {
  const turn = (at: string, prompt: string): ConvTurn => ({ prompt, at, ended_at: null, items: [] });
  const ev = (id: number, at: number) =>
    ({ id, session_id: 1, at, kind: 'stop_failure', detail: 'overloaded', claude_session_id: 'a' });
  const t0 = Date.parse('2026-09-18T10:00:00Z') / 1000;
  it('places events after the turn they follow', () => {
    const rows = buildThread(
      [turn('2026-09-18T10:00:00Z', 'a'), turn('2026-09-18T10:05:00Z', 'b')],
      [ev(1, t0 + 60), ev(2, t0 + 400)],
      { blocked: false },
      false,
    );
    expect(rows.map((r) => (r.kind === 'turn' ? r.turn.prompt : `e${r.event.id}`))).toEqual(['a', 'e1', 'b', 'e2']);
  });
  it('drops events older than the first loaded turn when truncated', () => {
    const rows = buildThread([turn('2026-09-18T10:00:00Z', 'a')], [ev(1, t0 - 60)], { blocked: false }, true);
    expect(rows).toHaveLength(1);
  });
  it('keeps them first when not truncated', () => {
    const rows = buildThread([turn('2026-09-18T10:00:00Z', 'a')], [ev(1, t0 - 60)], { blocked: false }, false);
    expect(rows[0].kind).toBe('event');
  });
});

describe('lastEventLabel / mergeEvents / statusChip', () => {
  it('labels the newest notable event', () => {
    const now = 1_000_000 * 1000;
    const evs = [
      { id: 1, session_id: 1, at: 999_700, kind: 'compact_done', detail: 'auto', claude_session_id: 'a' },
      { id: 2, session_id: 1, at: 999_900, kind: 'turn_done', detail: null, claude_session_id: 'a' },
    ];
    expect(lastEventLabel(evs, now)).toMatch(/^\/compact /);
  });
  it('merges by id, oldest first', () => {
    const a = { id: 2, session_id: 1, at: 5, kind: 'x', detail: null, claude_session_id: 'a' };
    const b = { id: 1, session_id: 1, at: 4, kind: 'y', detail: null, claude_session_id: 'a' };
    expect(mergeEvents([a], [b, a]).map((e) => e.id)).toEqual([1, 2]);
  });
  it('status chip prefers compacting', () => {
    expect(statusChip({ claude_status: 'working', current_activity: 'compacting' })).toBe('compacting');
    expect(statusChip({ claude_status: 'idle', current_activity: null })).toBe('idle');
  });
});
```

- [ ] **Step 4: Run** `npx vitest run src/lib/conversation.test.ts src/lib/live_events.test.ts`, then `npx svelte-check` and the full `npx vitest run` → PASS.

- [ ] **Step 5: Commit**

```bash
git -C "$WT" commit -am "feat(conversation-ui): live event fan-out and header/thread helpers"
```

---

### Task 3: `ConversationHeader.svelte`

**Files:**
- Create: `src/lib/ConversationHeader.svelte`, `src/lib/ConversationHeader.test.ts`

**Interfaces:**
- Consumes: Task 2 helpers.
- Produces: component props

```ts
{
  session: SessionRow;
  conversations: ConversationSummary[];
  /** claude_session_id being viewed; null = the current conversation. */
  viewing: string | null;
  /** From lastEventLabel(); null hides the slot. */
  lastEvent: string | null;
  /** A newer conversation started while an earlier one is being viewed. */
  newerAvailable: boolean;
  onSelect: (claudeSessionId: string | null) => void;
}
```

Test ids: `conv-header`, `conv-switcher` (button), `conv-switcher-menu` (list), `conv-switcher-item` (each option; `data-current="true|false"`), `conv-switcher-dot`, `conv-ctx` (meter, `role="meter"`, `data-level`, `data-stale`), `conv-model`, `conv-status`, `conv-last-event`.

- [ ] **Step 1: Failing component tests** (`ConversationHeader.test.ts`, same render helpers as `ConversationPanel.test.ts`):

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ConversationHeader from './ConversationHeader.svelte';
import type { SessionRow } from './sessions';
import type { ConversationSummary } from './conversation';

const session = (o: Partial<SessionRow> = {}) =>
  ({ id: 1, claude_session_id: 'bbb', claude_status: 'idle', current_activity: null, model: 'claude-opus-5',
     context_pct: 21, context_tokens: 42_000, context_window: 200_000, context_stale: false, ...o }) as SessionRow;
const conv = (o: Partial<ConversationSummary>): ConversationSummary => ({
  id: 1, session_id: 1, claude_session_id: 'aaa', transcript_path: null, started_at: 1_789_000_000,
  ended_at: 1_789_000_500, start_source: 'fleet', end_reason: 'clear', model: null, first_prompt: 'fix the bug',
  turns: 3, compactions: 0, current: false, ...o,
});
const list = [conv({ id: 2, claude_session_id: 'bbb', start_source: 'clear', current: true, ended_at: null, turns: 1, first_prompt: 'next' }), conv({})];

describe('ConversationHeader', () => {
  it('shows the current conversation, context, model and status', () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: '/compact 3m ago', newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-switcher').textContent).toContain('Current · /clear · 1 turn');
    expect(screen.getByTestId('conv-ctx').textContent).toContain('42k / 200k · 21%');
    expect(screen.getByTestId('conv-model').textContent).toContain('opus-5');
    expect(screen.getByTestId('conv-status').textContent).toContain('idle');
    expect(screen.getByTestId('conv-last-event').textContent).toContain('/compact 3m ago');
  });

  it('lists earlier conversations and selects one', async () => {
    const onSelect = vi.fn();
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    const items = screen.getAllByTestId('conv-switcher-item');
    expect(items).toHaveLength(2);
    expect(items[1].textContent).toContain('fix the bug');
    await fireEvent.click(items[1]);
    expect(onSelect).toHaveBeenCalledWith('aaa');
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });

  it('selecting the current entry passes null; Escape closes the menu', async () => {
    const onSelect = vi.fn();
    render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: true, onSelect });
    expect(screen.getByTestId('conv-switcher-dot')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item')[0]);
    expect(onSelect).toHaveBeenCalledWith(null);
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'Escape' });
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });

  it('hides the meter while viewing an earlier conversation and dims a stale one', () => {
    const { unmount } = render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.queryByTestId('conv-ctx')).toBeNull();
    unmount();
    render(ConversationHeader, { session: session({ context_stale: true }), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-ctx').getAttribute('data-stale')).toBe('true');
  });

  it('shows 0 after a clear', () => {
    render(ConversationHeader, { session: session({ context_pct: 0, context_tokens: 0 }), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-ctx').textContent).toContain('0 / 200k · 0%');
  });

  it('works with an empty conversation list (phase-1 hub not yet reached)', () => {
    render(ConversationHeader, { session: session(), conversations: [], viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-switcher').textContent).toContain('Current');
  });
});
```

- [ ] **Step 2: Implement** `ConversationHeader.svelte`:

```svelte
<script lang="ts">
  import type { SessionRow } from './sessions';
  import {
    contextMeter,
    conversationTitle,
    statusChip,
    switcherEntries,
    type ConversationSummary,
  } from './conversation';
  import { contextColor, contextTint } from './attention';

  let {
    session,
    conversations,
    viewing,
    lastEvent,
    newerAvailable,
    onSelect,
  }: {
    session: SessionRow;
    conversations: ConversationSummary[];
    viewing: string | null;
    lastEvent: string | null;
    newerAvailable: boolean;
    onSelect: (claudeSessionId: string | null) => void;
  } = $props();

  let open = $state(false);
  let menu: HTMLUListElement | undefined = $state();

  const entries = $derived(switcherEntries(conversations));
  const shown = $derived(
    viewing === null
      ? entries.find((c) => c.current)
      : entries.find((c) => c.claude_session_id === viewing),
  );
  const title = $derived(shown ? conversationTitle(shown) : viewing === null ? 'Current' : 'Earlier conversation');
  const meter = $derived(viewing === null ? contextMeter(session) : null);
  const model = $derived((session.model ?? shown?.model ?? '').replace(/^claude-/, ''));
  const status = $derived(viewing === null ? statusChip(session) : null);

  function pick(c: ConversationSummary) {
    open = false;
    onSelect(c.current ? null : c.claude_session_id);
  }
  function onMenuKey(e: KeyboardEvent) {
    if (e.key === 'Escape') open = false;
  }
  $effect(() => {
    if (open) menu?.focus();
  });
</script>

<div class="conv-header" data-testid="conv-header">
  <div class="switcher-wrap">
    <button
      type="button"
      class="switcher"
      data-testid="conv-switcher"
      aria-haspopup="listbox"
      aria-expanded={open}
      onclick={() => (open = !open)}
      >{title}{#if newerAvailable}<span class="dot" data-testid="conv-switcher-dot" title="A newer conversation started"></span>{/if}<span class="caret">▾</span></button
    >
    {#if open}
      <ul class="menu" role="listbox" tabindex="-1" data-testid="conv-switcher-menu" bind:this={menu} onkeydown={onMenuKey}>
        {#each entries as c (c.id)}
          <li
            role="option"
            aria-selected={viewing === null ? c.current : c.claude_session_id === viewing}
            data-testid="conv-switcher-item"
            data-current={c.current}
            onclick={() => pick(c)}
            onkeydown={(e) => e.key === 'Enter' && pick(c)}
            tabindex="0"
          >
            <span class="t">{conversationTitle(c)}</span>
            {#if c.first_prompt}<span class="p">{c.first_prompt}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
  <div class="facts">
    {#if meter}
      <span
        class="ctx"
        data-testid="conv-ctx"
        data-level={meter.level}
        data-stale={meter.stale}
        role="meter"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.round(meter.pct)}
        aria-label="context usage"
        title={meter.title}
        style="color: {contextColor(meter.level)}; border-color: {contextTint(meter.level)};"
        ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, meter.pct))}%; background: {contextColor(meter.level)};"></span><span class="ctx-pct">{meter.label}</span></span
      >
    {/if}
    {#if model}<span class="chip" data-testid="conv-model">{model}</span>{/if}
    {#if status}<span class="chip" data-testid="conv-status" data-status={status}>{status}</span>{/if}
    {#if lastEvent}<span class="muted" data-testid="conv-last-event">{lastEvent}</span>{/if}
  </div>
</div>
```

Styles: a sticky single-row bar (`position: sticky; top: 0; z-index: 2; background: var(--bg-pane); border-bottom: 1px solid var(--border)`), `.facts` right-aligned with `gap: 8px`, `.menu` an absolutely positioned popover (`background: var(--bg); border: 1px solid var(--border); max-height: 50vh; overflow: auto; min-width: 280px`), `.p` one line with ellipsis in `var(--fg-muted)`, `.dot` an 8px `var(--accent)` circle, `.ctx[data-stale='true'] { opacity: 0.55 }`, `.chip[data-status='compacting'], .chip[data-status='blocked'] { color: #e6a23c }`, `.chip[data-status='failed'] { color: #e64a4a }`. Move the `.ctx` / `.ctx-bar` / `.ctx-pct` rules from `ConversationPanel.svelte` here (Task 4 deletes them there). The header wraps to two rows under 520 px (`flex-wrap: wrap`).

Close the menu on an outside click: a `$effect` that, while `open`, adds a `pointerdown` listener on `document` closing it when the target is outside `.switcher-wrap` (remove it in the effect cleanup).

- [ ] **Step 3: Run** `npx vitest run src/lib/ConversationHeader.test.ts`, `npx svelte-check` → PASS (fix a11y warnings svelte-check reports; the test output must be warning-free). **Commit:**

```bash
git -C "$WT" commit -am "feat(conversation-ui): ConversationHeader with switcher, context meter, model, status and last event"
```

---

### Task 4: `ConversationPanel` — header, conversation switching, inline rows

**Files:**
- Modify: `src/lib/ConversationPanel.svelte`, `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `ConversationHeader` (Task 3); `listConversations`, `sessionConversation(id, turns, claudeSessionId?)`, `buildThread`, `mergeEvents`, `lastEventLabel`, `SOURCE_LABELS`, extended `groupItems`, `onTimelineEvent`, `onConversationsChanged` (Task 2); `Conversation.events` (Task 1).

Behaviour (spec §2.1–2.2), all required:

1. **Header** mounted at the top of `.conversation-panel` (above `.thread-area`), props: `session`, `conversations`, `viewing`, `lastEvent = lastEventLabel(events, nowMs)`, `newerAvailable`, `onSelect`.
2. **Conversation list**: `conversations = $state<ConversationSummary[]>([])`; `loadConversations()` calls `listConversations(session.id)` (ignore errors: keep the old list), runs on session reset and on `onConversationsChanged(session.id, …)` (registered in a `$effect` keyed on `sessionId`, cleaned up on change/unmount).
3. **Viewing an earlier conversation**: `viewing = $state<string | null>(null)`. `onSelect(id)` sets `viewing = id`, clears `conv`, `expanded`, `unseen`, resets `turnsWanted`, bumps `seq`, and calls `load()`. `load()` passes `viewing ?? undefined` as `claudeSessionId`. While `viewing !== null`:
   - a banner above the thread: `<div class="viewing" data-testid="conv-viewing-banner">Viewing an earlier conversation · <button data-testid="conv-back-current">Back to current</button></div>`; the button calls `onSelect(null)`;
   - the 5 s poll and the `turn_seq` refetch skip (`if (viewing !== null) return;` at the top of both);
   - the composer textarea and Send are `disabled`, and the composer status shows `Viewing an earlier conversation — go back to current to send.`;
   - the activity indicator / pending block are hidden.
4. **Following `/clear` / `/resume`**: a `$effect` on `session.claude_session_id` (not on `session.id`; the existing id-reset effect stays):
   - when it changes and `viewing === null`: reset the thread state exactly like the id reset does for `conv`, `errorCode`, `errorMsg`, `expanded`, `atBottom`, `unseen`, `turnsWanted`, `loadingOlder`, `pending`, probe, `sentTurnSeq`, `idleSeenSinceSend` — but keep `draft`; then `load()`, `loadConversations()`, and set `switchNotice = { source }` where `source` is the new current entry's `start_source` from the reloaded list (fallback `'unknown'`). Render it as `<div class="switch-notice" data-testid="conv-switch-notice">New conversation ({SOURCE_LABELS[source]}) · <button data-testid="conv-view-previous">View previous</button> <button aria-label="Dismiss" data-testid="conv-switch-dismiss">×</button></div>`. "View previous" selects the most recent non-current entry. The notice clears on dismiss, on the next session reset, or when the user sends a prompt.
   - when it changes and `viewing !== null`: only `newerAvailable = true` (cleared when the user goes back to current).
   - the first value seen for a session (after the id reset) must not trigger a notice.
5. **Events**: `events = $derived(mergeEvents(conv?.events ?? [], pushed))` where `pushed = $state<SessionEvent[]>([])` collects `onTimelineEvent(session.id, e => …)` pushes whose `claude_session_id === (viewing ?? session.claude_session_id)`. Reset `pushed` whenever `conv` is reset.
6. **Thread rendering**: replace `{#each conv.turns as turn, i}` with `{#each buildThread(conv.turns, events, { blocked: indicator?.kind === 'blocked' }, conv.truncated) as row (row.kind === 'turn' ? `t${row.index}` : `e${row.event.id}`)}`; turn rows render the existing turn markup with `row.turn` / `row.index`; event rows render
   `<div class="event" data-testid="conv-event" data-tone={row.event.tone}><span class="label">{row.event.label}</span>{#if row.event.detail}<span class="detail">{row.event.detail}</span>{/if}<time>{relativeFromUnix(row.event.at, nowMs)}</time></div>` (thin, centred, muted; `warn` amber, `error` red).
7. **New item groups** inside a turn's reply:
   - `compact`: `<details class="compact" data-testid="conv-compact"><summary>Compacted ({trigger ?? 'unknown'}){#if pre_tokens} · was {formatTokens(pre_tokens)} tokens{/if}</summary>{#if summary}<Markdown source={summary} />{:else}<p class="muted">Summary not in the loaded tail.</p>{/if}</details>` — closed by default;
   - `command`: `<div class="command" data-testid="conv-command"><code>{name}{args ? ` ${args}` : ''}</code>{#if output}<pre class="command-out">{output}</pre>{/if}</div>` (`pre` clamped to 8 lines with the existing "Show more" pattern if longer);
   - `interrupt`: `<div class="interrupt" data-testid="conv-interrupt">Interrupted{during_tool ? ' during a tool call' : ''}</div>`.
   A turn with `prompt === null` whose first group is `command` or `compact` renders no prompt block (already true for null prompts) — verify.
8. **Context meter**: delete the composer-foot meter markup and its CSS (the header owns `conv-ctx` now). Keep `ctxLevel` / `suggestCompact` for the compact chip.
9. **Empty states**: an earlier conversation whose transcript is gone (`E_NO_TRANSCRIPT` while `viewing !== null`) shows `Transcript no longer on host` (`data-testid="conv-empty"`) with the back button still available.

- [ ] **Step 1: Failing panel tests** in `ConversationPanel.test.ts` (reuse its `session()`, `conv()`, `ok()`, `err()` helpers and the `vi.mock('./conversation', …)` pattern; also mock `listConversations`). Required cases:

```ts
  it('renders the header with the current conversation and the context meter', async () => { /* mock listConversations → [current]; expect conv-header, conv-ctx text '42k / 200k · 21%' */ });
  it('switches to an earlier conversation read-only and back', async () => {
    /* click conv-switcher → pick the earlier item → sessionConversation called with (id, turns, 'aaa');
       conv-viewing-banner visible; conv-composer-input disabled; advancing the poll interval does NOT refetch;
       click conv-back-current → sessionConversation called with claudeSessionId undefined; banner gone; input enabled */
  });
  it('follows a /clear: new claude_session_id resets the thread and shows the notice', async () => {
    /* render with claude_session_id 'aaa'; rerender with 'bbb' (component.$set or re-render helper used in this file);
       expect a fresh sessionConversation call, conv-switch-notice text 'New conversation (/clear)' (listConversations returns bbb current with start_source 'clear');
       draft text preserved */
  });
  it('does not show the notice on first mount or on a session switch', async () => { /* … */ });
  it('while viewing an earlier conversation, a new id only lights the switcher dot', async () => { /* … conv-switcher-dot present, still viewing */ });
  it('renders compact, command and interrupt items', async () => {
    /* conv() with items [{kind:'command',name:'/model',args:'opus',output:'Set model to opus'},
       {kind:'compact',trigger:'auto',pre_tokens:180000,summary:'S'}, {kind:'interrupt',during_tool:true}] →
       conv-command text '/model opus' and 'Set model to opus'; conv-compact summary 'Compacted (auto) · was 180k tokens';
       conv-interrupt 'Interrupted during a tool call' */
  });
  it('interleaves timeline events and appends pushed ones', async () => {
    /* conv().events = [stop_failure after turn 1]; expect conv-event 'Turn failed: …' after the first conv-prompt;
       then dispatchTimelineEvents([{…permission_prompt, session_id: 1, claude_session_id: current}]) (import from ./live_events)
       and a blocked session → conv-event 'Waiting for permission' appears without a refetch;
       a pushed event for another claude_session_id is ignored */
  });
  it('refreshes the conversation list on session:conversations', async () => { /* dispatchConversationsChanged([1]) → listConversations called again */ });
  it('shows "Transcript no longer on host" for a vanished earlier transcript', async () => { /* … */ });
```

Write each body fully (the comments above state the exact assertions); follow the file's existing async settling pattern (`await tick(); await Promise.resolve(); await tick();`). Update the existing tests that asserted `conv-ctx` inside the composer foot to find it in the header.

- [ ] **Step 2: Run to fail**, **Step 3: implement** per the behaviour list, **Step 4: run** `npx vitest run src/lib/ConversationPanel.test.ts`, then full `npx vitest run` and `npx svelte-check` (0 errors, 0 new warnings).

- [ ] **Step 5: Commit**

```bash
git -C "$WT" commit -am "feat(conversation-ui): header, earlier-conversation view, /clear follow, inline events and new item kinds"
```

---

### Task 5: Timeline push, docs, verification

**Files:**
- Modify: `src/lib/Timeline.svelte`; Create: `src/lib/Timeline.test.ts`
- Modify: `src/lib/timeline.ts` (labels for the new kinds)
- Modify: `docs/superpowers/specs/2026-09-18-conversation-events-design.md` (mark Phase 2 implemented; note the compact-event rule: inline compaction comes from the transcript item, not from `compact_*` events)

- [ ] **Step 1: Failing Timeline test**

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
vi.mock('./timeline', async () => {
  const actual = await vi.importActual<typeof import('./timeline')>('./timeline');
  return { ...actual, sessionHistory: vi.fn() };
});
import Timeline from './Timeline.svelte';
import { sessionHistory } from './timeline';
import { dispatchTimelineEvents } from './live_events';

const ev = (id: number, kind: string, session_id = 7) =>
  ({ id, session_id, at: 1_789_000_000 + id, kind, detail: null, claude_session_id: 'a' });

describe('Timeline', () => {
  it('prepends a pushed event for its session without refetching', async () => {
    vi.mocked(sessionHistory).mockResolvedValue({ ok: true, value: [ev(1, 'turn_done')] } as never);
    render(Timeline, { sessionId: 7 });
    await tick(); await Promise.resolve(); await tick();
    expect(sessionHistory).toHaveBeenCalledTimes(1);
    dispatchTimelineEvents([ev(2, 'compact_done'), ev(3, 'turn_done', 8)]);
    await tick();
    const kinds = [...document.querySelectorAll('li.ev')].map((li) => li.getAttribute('data-kind'));
    expect(kinds).toEqual(['compact_done', 'turn_done']);
    expect(sessionHistory).toHaveBeenCalledTimes(1);
  });
});
```

(Match the real `Result` shape used by `timeline.ts`; adjust the mock value accordingly.)

- [ ] **Step 2: Implement**: in `Timeline.svelte` add a `$effect` keyed on `sessionId` that registers `onTimelineEvent(sessionId, e => { if (!events.some(x => x.id === e.id)) events = [e, ...events].slice(0, 500); })` and returns the unsubscribe. Keep the `refreshKey` effect (fallback for a desktop talking to an older hub). In `timeline.ts`: `eventCategory` maps `conversation_started` / `conversation_ended` / `compact_started` / `compact_done` / `turn_done` to `turns`; `kindLabel` stays generic. Add those cases to `timeline.test.ts`.

- [ ] **Step 3: Full verification** (unpiped): `scripts/ci-local.sh`. Expected: all stages green.

- [ ] **Step 4: Manual check** (controller, dev build `pnpm tauri dev` if runnable; otherwise record as not run): open a session's Conversations tab, run `/model`, `/compact`, `/clear`, `/resume`; confirm the header, switcher, notice, inline rows and read-only earlier view.

- [ ] **Step 5: Commit**

```bash
git -C "$WT" commit -am "feat(timeline): live timeline via session:event push; docs for phase 2"
```

---

## Self-Review Notes

- Spec §2.1 → Tasks 3–4 (switcher, meter incl. 0 after clear and stale dimming, model, status, last event, auto-follow + notice, dot while viewing earlier, reset key on `claude_session_id`); §2.2 → Tasks 2 (mapping) and 4 (rendering); §2.3 → Task 1; §2.4 → Task 5. Edge cases: empty `/clear`-after-`/clear` conversations hidden (`switcherEntries`), vanished transcript (Task 4 item 9), old hub without phase 1 (header with an empty list; `events` absent → `conv?.events ?? []`).
- Compaction is shown from the transcript item, not from `compact_*` events (avoids duplicates); the header's status chip shows `compacting` while `PreCompact` is in flight.
- `api_error` system entries are not rendered (StopFailure events cover failed turns). Phase 3 (tool-call lines, subagents, "doing now", find, copy, turn index) is out of scope; the existing "↓ N new" pill already covers part of phase 3 navigation.
