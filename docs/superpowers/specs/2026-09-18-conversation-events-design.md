# Conversation event tracking and Conversations tab UX — design

- **Date:** 2026-09-18
- **Status:** draft, awaiting review
- **Builds on:** `docs/specs/2026-09-11-session-management-analysis.md` (D4, D11, P3),
  `docs/superpowers/specs/2026-09-15-hook-events-design.md`

## Goal

Fleet tracks the lifecycle of the Claude Code *conversation* running inside a
session, not only the session. After `/clear`, `/resume` or `/compact`, the
Conversations tab, the context meter and the timeline reflect what happened
within a second, driven by hooks. The Conversations tab becomes a
readable, navigable view of the current conversation with a switcher for the
earlier ones.

## Problems today

1. **No rebind after `/clear`.** `claude_session_id` changes only when
   reconcile's `claude agents --json` match succeeds (name, or cwd when it is
   unique). Fleet's own sessions don't pass `--name`, so two agents in one cwd
   never rebind (D4).
2. **Hooks from the new conversation are dropped.** `service/hooks.rs` resolves
   the row by `claude_session_id` only. Until reconcile rebinds, Stop /
   UserPromptSubmit from the new id find no row.
3. **Stale transcript wins.** `transcript::read_script` prefers the stored
   `transcript_path` whenever the file exists, and nothing clears it when the
   id changes. The old `.jsonl` survives `/clear`, so the tab keeps showing the
   old conversation.
4. **Context never resets.** `context_pct` comes only from the pane footer and
   is upserted with `COALESCE(excluded.context_pct, context_pct)`. After
   `/clear` the footer often shows no percentage, so the old value sticks.
5. **Unobserved events.** `SessionStart` is not installed (it cannot be an
   `http` hook). `SessionEnd(clear|resume)` is excluded by the matcher.
   `PreCompact` / `PostCompact` are not installed.
6. **Parser and timeline gaps.** `parse_conversation` renders a compact summary
   as a prompt turn and doesn't recognise slash-command entries. Timeline
   refetches only when certain row fields change; there is no push for a new
   event.
7. **Task side effect.** `service/tasks.rs` fails a dispatched task when the
   worker's `claude_session_id` changes, so `/clear` inside a worker fails its
   task.

## Non-goals

- Subagent (Task tool) lifecycle hooks and per-subagent cost (D18).
- Per-tool hook spooling (`PreToolUse` / `PostToolUse` for every tool). Tool
  activity keeps coming from the transcript.
- Editing or deleting transcripts. Fleet only reads them.
- Changing how sessions are created or launched.

## Delivery

Three PRs, each shippable on its own:

| Phase | Scope |
|---|---|
| 1 — Backend | Hooks, pane binding, `conversations` table, rebind, context computation, push event, task fix |
| 2 — Conversations UI | Header, conversation switcher, inline events, parser fixes |
| 3 — Detail UX | Compact tool calls, subagents, "doing now", navigation |

---

## Phase 1 — Backend

### 1.1 Hook installation (`service/hooks_install.rs`)

`FLEET_HOOK_EVENTS` grows to:

| Event | Matcher | Handler type | Why |
|---|---|---|---|
| `Stop` | — | http | unchanged |
| `UserPromptSubmit` | — | http | unchanged |
| `PostToolUse` | `EnterWorktree\|ExitWorktree` | http | unchanged |
| `SessionEnd` | `logout\|prompt_input_exit\|other\|clear\|resume` | http | `clear` / `resume` now close the conversation (not the session) |
| `StopFailure` | — | http | unchanged |
| `Notification` | unchanged | http | unchanged |
| `SessionStart` | — (all sources) | **command** | opens a conversation; carries `source`, `model`, `transcript_path` |
| `PreCompact` | — | http | "compacting" status and timeline event |
| `PostCompact` | — | http | context marked stale until the next usage; timeline event |

**Pane header.** Every http entry gains
`"X-Fleet-Pane": "$TMUX_PANE"` in `headers` and `"allowedEnvVars": ["TMUX_PANE"]`.
Outside tmux the variable is empty and the header arrives empty; that is a
normal "no pane" case.

**SessionStart command hook.** `SessionStart` accepts only `command` /
`mcp_tool` handlers, so fleet installs:

```sh
curl -sS -m 5 -o /dev/null -X POST \
  -H @"$HOME/.claude/fleet-hook.headers" \
  -H "X-Fleet-Pane: ${TMUX_PANE:-}" \
  -H 'Content-Type: application/json' \
  --data-binary @- '<hook_url>' || true
```

- `"async": true`, `"timeout": 5`. A down server never stalls startup; `|| true`
  keeps the exit code 0 so Claude Code shows no hook error.
- The bearer token lives in `~/.claude/fleet-hook.headers` (one line,
  `Authorization: Bearer <token>`, mode 0600), written by the same code paths
  that write `settings.json` (`auto_install_local_hook`, `provision_hook`).
  The token never appears in argv (SEC-3) or in `settings.json` for this
  entry.
- `<hook_url>` is shell-quoted with `crate::shell::quote`.
- Recognition: `is_fleet_command_hook_entry` matches a `command` entry whose
  command contains `fleet-hook.headers` and ends at a `/hook` URL. The legacy
  pre-Track-B stripping logic (token in the URL) must not match it; a unit test
  pins that.
- Hosts without `curl`: provisioning checks `command -v curl` and skips the
  SessionStart entry with a warning. The fallback rebind (1.4) still covers
  those hosts.

Hosts pick up the new entries on the next `provision_hosts`; the local host
on app start.

### 1.2 Payload and routing (`mcp/hooks.rs`, `service/hooks.rs`)

`HookPayload` gains `source`, `model`, `trigger` (`manual` | `auto`) and
`custom_instructions` (read, never stored). `handle_hook` reads
`X-Fleet-Pane` into `HookContext { host_alias, pane_id: Option<String> }`.

**Row resolution** (`resolve_hook_row`), in order:

1. `(caller host, pane_id)` when the header is non-empty and matches exactly one
   live row's `tmux_pane_id`.
2. `claude_session_id = payload.session_id`.
3. For `SessionStart` / `UserPromptSubmit` only: a row on the caller host
   marked `awaiting_rebind` (set by `SessionEnd(clear|resume)`, 1.3) whose cwd
   equals `payload.cwd`, when exactly one such row exists.
4. Otherwise: no row, the hook is a no-op (as today).

The master token caller has no host; it resolves by step 2 only.

**Rebind.** When the resolved row's `claude_session_id` differs from
`payload.session_id` and the event is `SessionStart` (not `compact`) or
`UserPromptSubmit`, the handler runs `rebind_conversation(row, new_id, source,
transcript_path, model)` (1.3) before applying the event — subject to the
eligibility rule below.

**Rebind eligibility.** Every `claude` started in a pane inherits
`$TMUX_PANE`, a Bash-tool `claude -p` included, so a pane match (step 1) alone
does not prove the payload's conversation replaced the row's. A row found by
its pane moves to a new id only when one of these holds:

- (a) the row's `claude_session_id` is NULL;
- (b) the row is awaiting a rebind within the TTL (`SessionEnd(clear|resume)`
  just fired for its current conversation);
- (c) the row's current conversation has ended (`ended_at` set), or its
  `claude_status` is `stopped`;
- (d) the event is `SessionStart` with source `clear` (only the interactive
  session emits it). Not `resume`: a nested `claude -p --resume <id>` / `-c`
  starts with it too; an interactive `/resume` rebinds through (b), since its
  `SessionEnd(resume)` sets the awaiting mark first.

Otherwise a pane-resolved event carrying a non-current id is the row's own
earlier conversation when the row has a conversation by that id (it updates
only that conversation — see *Edge cases*: `/clear` mid-turn), and else a
foreign `claude` sharing the pane: a no-op with no status, turn, timeline,
conversation or task effect. Step 3 matches only rows awaiting a rebind, so it
is always eligible; step 2 matches only the current id.

**UserPromptSubmit rebind source.** The SessionStart command hook is async and
may land after the first `UserPromptSubmit` (a fleet `/clear` followed quickly
by `send_prompt`). A `UserPromptSubmit` rebind onto a row awaiting one takes its
source from the just-closed conversation's `end_reason` (`clear` → `clear`,
`resume` → `resume`); otherwise `source = 'unknown'`, which covers CLIs or
hosts where the SessionStart command hook is missing. It keeps `last_prompt` /
`current_activity` (the turn is starting) while a resetting source still zeroes
the context.

`SessionStart` with the *current* id (a fleet-created session starting with its
own `--session-id`, or one that lost the race to its `UserPromptSubmit`) does
not rebind; it ensures the conversation is open, upgrades a `start_source` of
`unknown` to its own source, and applies the source's resets (1.3 steps 3–4) —
unless a turn has already begun on that conversation (`turns > 0` or
`first_prompt` set, both written only by the conversation's own hooks), in
which case it resets nothing.

### 1.3 Conversation model

**Migration `036_conversations.sql` (numbered 034 before main took 034/035):**

```sql
CREATE TABLE conversations (
  id                INTEGER PRIMARY KEY,
  session_id        INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  claude_session_id TEXT    NOT NULL,
  transcript_path   TEXT,
  started_at        INTEGER NOT NULL,  -- epoch seconds, like every *_at column
  ended_at          INTEGER,
  start_source      TEXT    NOT NULL,  -- startup|resume|clear|compact|fork|fleet|unknown
  end_reason        TEXT,              -- clear|resume|logout|prompt_input_exit|other|replaced|killed
  model             TEXT,
  first_prompt      TEXT,              -- first 200 chars, for the switcher
  turns             INTEGER NOT NULL DEFAULT 0,
  compactions       INTEGER NOT NULL DEFAULT 0,
  UNIQUE (session_id, claude_session_id)
);
CREATE INDEX conversations_by_session ON conversations(session_id, started_at DESC);

ALTER TABLE sessions ADD COLUMN tmux_pane_id TEXT;
ALTER TABLE sessions ADD COLUMN awaiting_rebind_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN context_window INTEGER;
ALTER TABLE sessions ADD COLUMN context_source TEXT;      -- transcript|hook|pane
ALTER TABLE sessions ADD COLUMN context_at INTEGER;
ALTER TABLE sessions ADD COLUMN context_stale INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN model TEXT;
ALTER TABLE session_events ADD COLUMN claude_session_id TEXT;

-- Backfill: one open conversation per row that already has an id.
INSERT OR IGNORE INTO conversations (session_id, claude_session_id, transcript_path,
                                     started_at, start_source)
  SELECT id, claude_session_id, transcript_path, created_at, 'unknown'
  FROM sessions WHERE claude_session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (36);
```

Registered in `MIGRATIONS`, gated, run inside a transaction.

**Invariant:** each session has at most one open conversation
(`ended_at IS NULL`), and it is the one whose `claude_session_id` equals the
session's.

**`store::conversations`:**

- `open_conversation(session_id, claude_session_id, source, transcript_path, model, at)`:
  - If a row for `(session_id, claude_session_id)` exists (a `/resume` back to
    an earlier conversation), reopen it: clear `ended_at` and `end_reason`, keep
    `started_at`. No duplicate.
  - Otherwise insert.
  - In both cases close any other open conversation of the session with
    `end_reason = COALESCE(pending_end_reason, 'replaced')`.
- `close_conversation(session_id, claude_session_id, reason, at)`.
- `list_conversations(session_id, limit)`, newest first.
- `bump_turns(session_id, claude_session_id)`; `set_first_prompt_if_null(…)`;
  `bump_compactions(…)`.

**`rebind_conversation`** (service layer, one store transaction):

1. `open_conversation(...)`.
2. Update `sessions`: `claude_session_id`, `transcript_path` (the payload path
   when it validates as `<session_id>.jsonl`, else `NULL` so the read script
   falls back to the cwd-derived path), `model`, `awaiting_rebind_at = NULL`.
3. For `source ∈ {startup, clear, fleet}`: reset per-conversation fields:
   `context_tokens = 0`, `context_stale = 0`, `context_source = 'hook'`,
   `context_at = now`, `context_pct = 0`, `current_activity = NULL`,
   `last_prompt = NULL`. `turn_seq` keeps counting (it is a change signal, not a
   per-conversation count).
4. For `source = resume`: `context_stale = 1` (the size is unknown until the
   next usage line), other fields untouched.
5. Timeline event `conversation_started` (`detail`: `{source, claude_session_id,
   model}`).
6. Emit `SessionUpdated` and `ConversationsChanged(session_id)` after commit.

**Per-event effects:**

| Event | Effect |
|---|---|
| `SessionStart(startup\|clear\|resume\|fork)` | rebind (above) |
| `SessionStart(compact)` | same id: `bump_compactions`, `context_stale = 1`, timeline `compact_done` |
| `SessionEnd(clear\|resume)` | `close_conversation(reason)`, set `awaiting_rebind_at = now`; row status stays as is (not `stopped`); timeline `conversation_ended` |
| `SessionEnd(logout\|prompt_input_exit\|other)` | as today (→ `stopped`) plus `close_conversation(reason)` |
| `PreCompact` | `current_activity = 'compacting'`, timeline `compact_started` with `trigger` |
| `PostCompact` | `bump_compactions`, `context_stale = 1`, timeline `compact_done` |
| `UserPromptSubmit` | as today plus `set_first_prompt_if_null` |
| `Stop` / `StopFailure` | as today plus `bump_turns` and a context refresh (1.5) |

`SessionStart(compact)` and `PostCompact` both fire on compaction on recent CLIs.
The handler dedupes: a `compact_done` within 10 s for the same conversation is
skipped.

`awaiting_rebind_at` older than 5 minutes is ignored by resolution step 3, so a
stale mark cannot capture an unrelated session later.

### 1.4 Fallback rebind (no hooks / old CLI)

- Reconcile's existing `claude agents --json` path, when it changes
  `claude_session_id`, calls `rebind_conversation(..., source = 'unknown')`
  instead of the bare `COALESCE` write. The `store/reconcile.rs` upsert stops
  writing `claude_session_id` directly; the rebind helper owns it.
- The same helper clears `transcript_path` when the id changes. This alone fixes
  problem 3 on unprovisioned hosts.
- Reconcile records `tmux_pane_id` for every row from the existing
  `list-panes` output (add `#{pane_id}` to the format). A tmux server restart
  gives new pane ids; reconcile overwrites them each pass.
- Sessions created by fleet open their first conversation with
  `start_source = 'fleet'` at creation, since fleet chose the `--session-id`.

### 1.5 Context size

**Source of truth: the transcript.** The last assistant entry's
`message.usage` gives the prompt size of the latest request:

```
context_tokens = input_tokens + cache_read_input_tokens + cache_creation_input_tokens
```

(`output_tokens` is excluded: it becomes input only on the next request.)

`context_window` comes from the model: a `[1m]` suffix or a known 1M model
means 1 000 000, everything else 200 000. The table lives in one function,
`context_window_for(model)`, in `service/pane_intel.rs` next to the status
enums.

**When it is computed:**

- On `Stop` / `StopFailure`: a small tail read (last 64 KiB) of the conversation's
  transcript, off the hook's response path (spawned task), writes
  `context_tokens`, `context_window`, `context_pct`, `context_source =
  'transcript'`, `context_stale = 0`.
- On `session_conversation` reads: the parser already walks the tail; it
  returns the same numbers in the `Conversation` payload, and the command writes
  them back when newer than `context_at`.
- On `/clear` / startup: 0 from the hook (1.3).

**Precedence.** The reconcile upsert replaces
`COALESCE(excluded.context_pct, context_pct)` with: take the pane-footer value
only when `context_source` is `pane` or `NULL`, or when `context_at` is older
than the pane reading and older than 2 minutes. A transcript or hook value is
never overwritten by an older or missing pane value, and a missing pane value
never keeps an old number after a reset.

`context_pct` stays on the wire for compatibility and is derived:
`round(100 * context_tokens / context_window)` when tokens are known.

### 1.6 Events and push

- New `RowChange::SessionEventAdded(SessionEvent)` emitted by
  `insert_session_event` callers after commit. Desktop forwards it as
  `session:event_added`; hub SSE (`/events`) forwards it for `full` and
  `readonly` clients.
- New `RowChange::ConversationsChanged { session_id }` (payload is the id only;
  the UI refetches the small list).
- `Stop` now also writes a `turn_done` timeline event (D11), with `detail`
  truncated to 200 chars of `last_assistant_message` when present.
- `session_events.claude_session_id` is filled for every hook-originated
  event so the UI can place events inside the right conversation.

### 1.7 Tasks

`service/tasks.rs` no longer fails a task when the worker's
`claude_session_id` changes *through a rebind with source `clear | resume |
compact`*. A change through `recreate_session` (source `fleet`) still fails it,
as today. The check reads the newest `conversation_started` event's source. A
tolerated switch re-stamps the task's `worker_claude_session_id` onto the new
id, so the next check compares against the conversation the worker is
actually on.

### 1.8 API surface

- Tauri command and MCP tool `session_conversations { session_id, limit? }` →
  `[{ claude_session_id, started_at, ended_at, start_source, end_reason, model,
  first_prompt, turns, compactions, current: bool }]`.
- `session_conversation` gains an optional `claude_session_id` argument to read
  an earlier conversation (validated: it must belong to the session). The
  response gains `context: { tokens, window, pct, stale } | null`,
  `model`, and `events: SessionEvent[]` for that conversation.
- `SessionRow` wire type gains `model`, `context_tokens`, `context_window`,
  `context_stale`; mirrored in `src/lib/*.ts` as `T | null`.
- Regenerate `docs/control-api-reference.md` (`REGEN_DOCS=1`).

---

## Phase 2 — Conversations UI

### 2.1 Header

A single sticky bar above the transcript:

- **Conversation switcher** (left): "Current · started 14:02 · 23 turns". The
  dropdown lists earlier conversations from `session_conversations`: start
  time, how it began (`/clear`, `/resume`, startup), first prompt (one line),
  turns. Picking one loads it read-only: a banner "Viewing an earlier
  conversation · Back to current" appears and the composer is disabled.
- **Context meter:** `42k / 200k · 21%`, coloured with `contextLevel`. A
  `stale` value renders dimmed with "after compact" as its tooltip. After
  `/clear` it shows `0`.
- **Model** chip, **status** chip (working / blocked / idle / compacting), and
  the **last event** ("/compact 3 min ago", "rate limited 1 min ago").

When a rebind arrives while the user is viewing the current conversation, the
panel switches to the new one and shows a divider toast "New conversation
(/clear)"; the previous one is one click away in the switcher. When the user is
viewing an earlier conversation, it stays put and the switcher shows a dot.

The panel's reset key changes from `session.id` to
`session.id + claude_session_id`, so a rebind resets the scroll, the cached
conversation and the in-flight sequence number.

### 2.2 Inline events

Events from `events[]` are merged into the turn list by timestamp and rendered
as thin centred rows, distinct from turns:

| Event | Row |
|---|---|
| `conversation_started(resume)` | "Resumed conversation" |
| `compact_started` / `compact_done` | "Compacting… (auto)" → "Compacted · 180k → 12k", with the summary collapsed under it |
| `stop_failure` | red: "Turn failed: rate limit" with details on expand |
| `notification(permission_prompt)` | amber: "Waiting for permission: Bash" while blocked; greyed once answered |
| interrupt (transcript `[Request interrupted by user]`) | "Interrupted" |
| slash command entries | "/model opus", "/context", shown as commands, not prompts |

### 2.3 Parser fixes (`service/transcript.rs::parse_conversation`)

- `isCompactSummary` user entries and `compact_boundary` system entries become
  a `compact` item carrying the summary text, not a prompt turn.
- `isMeta` entries and `<command-name>` / `<local-command-stdout>` wrappers
  become `command` items.
- The interrupt marker becomes an `interrupt` item.
- Output types gain those item kinds; the TS `ConvItem` union mirrors them.

### 2.4 Timeline

`Timeline.svelte` subscribes to `session:event_added` for its session and
prepends the event, instead of refetching on field changes. The field-based
`refreshKey` stays as a fallback for desktop builds talking to an older hub.

---

## Phase 3 — Detail UX

### 3.1 Tool calls

- One compact line per call: icon, verb and target ("Edit
  `src/store.rs`", "Bash `cargo test -p fleet-core`", "Read 3 files"),
  duration when the result carries it, and a red marker on error.
- Consecutive calls of the same tool collapse into one expandable group
  (exists today; restyled).
- Expanding shows the input (diff for Edit/Write, command for Bash) and the
  truncated result.
- `Task` / `Agent` calls render as a subagent block with the agent type,
  description and its final message; nested tool calls are not shown.

### 3.2 "Doing now"

While `claude_status = working`, the last row is a live line built from the
newest unfinished tool call in the transcript tail (falls back to the pane
spinner text that exists today).

### 3.3 Navigation

- **Jump to latest** button when scrolled up; a "N new" pill when turns
  arrive while scrolled up. Auto-scroll only when already at the bottom.
- **Find in conversation** (Cmd/Ctrl+F while the panel has focus): highlights
  matches, Enter / Shift+Enter to move.
- **Copy** on hover for a prompt, an assistant message and a tool result.
- **Turn index**: the header's turn count opens a list of prompts (first line)
  to jump to.

---

## Edge cases

| Case | Handling |
|---|---|
| `/clear` twice quickly | Each SessionStart opens a conversation; the empty middle one has `turns = 0` and is hidden from the switcher unless it is current. |
| `/clear` mid-turn | The old turn's `Stop` carries the old id. Resolution by pane finds the row; the id differs from the current, so the event is attributed to the old conversation (`bump_turns` on it) and does **not** rebind back. Rule: only `SessionStart` and `UserPromptSubmit` rebind; `Stop`, `StopFailure`, `Notification`, `PreCompact`, `PostCompact` with a non-current id update that conversation only. |
| Hook before transcript line | The context tail read retries once after 500 ms if the last assistant entry predates the Stop. |
| `/resume` to an earlier conversation | `open_conversation` reopens the existing row; no duplicate; context marked stale. |
| `/resume` to a conversation from another session's cwd | The id is new to this session; a new conversation row is created for this session. The same `claude_session_id` may exist under another session (UNIQUE is per session). |
| Fork (`--fork-session`) | Treated as a new conversation with `start_source = fork`. |
| Auto-compact during a turn | `PreCompact(auto)` sets `compacting`; the next `UserPromptSubmit` / `Stop` clears it. |
| tmux server restart | Pane ids change; hooks resolve by `claude_session_id` until the next reconcile stores the new pane id. |
| Discovered (non-fleet) session | Works: `$TMUX_PANE` is set in any tmux pane. Outside tmux there is no row to match. |
| `move_session` to another host | The new host's session starts a new conversation (`start_source = fleet`); the old conversation is closed with `end_reason = 'replaced'`. |
| Kill / recreate | `close_conversation(reason = 'killed')`; recreate opens a new one with `source = fleet`. |
| Unprovisioned host / old CLI | Fallback rebind from reconcile (1.4); context from the transcript on Stop is unavailable without hooks, so the read path and pane footer supply it. |
| Two agents in one cwd, no hooks | Still ambiguous (as today). The UI shows the conversation it has; no wrong rebind happens because reconcile refuses to match. |
| Hook from master token | Resolves by `claude_session_id` only; never rebinds through step 3. |
| Stale `awaiting_rebind_at` | Ignored after 5 minutes. |
| Transcript file deleted | The switcher entry stays; loading it shows "Transcript no longer on host". |
| Very long history | `session_conversations` caps at 50 by default; the store keeps all rows (they are small). |

## Error handling

- Hooks never fail loudly: every handler returns 2xx; errors are logged at
  `warn` and dropped (existing contract).
- The context tail read is best-effort; a failure leaves the previous value
  with its `context_at`, and the UI shows the age.
- `rebind_conversation` is one transaction; on error nothing changes and the
  next hook retries the rebind naturally.
- Timeline writes are best-effort (`let _ =`) as today.

## Testing

**Rust (fleet-core):**

- `hooks_install`: the new entry set; `X-Fleet-Pane` + `allowedEnvVars`; the
  SessionStart command entry quotes the URL, contains no token, and is not
  stripped by the legacy matcher; the headers file is written 0600.
- `resolve_hook_row`: pane hit, id hit, awaiting-rebind hit, ambiguous cwd miss,
  stale mark ignored, master token restricted to id.
- Rebind race matrix: `/clear` then old `Stop`; `/clear` twice; `/resume` back;
  `SessionStart(compact)` + `PostCompact` dedupe; fallback rebind from
  reconcile; kill/recreate.
- Context: usage arithmetic, window from model, precedence against pane values
  (newer transcript beats older pane, reset to 0 sticks when the footer is
  empty).
- Parser: compact summary, compact boundary, meta/command entries, interrupt.
- Migration 036: fresh DB and upgrade from 035 with backfill.
- Tasks: `/clear` inside a worker keeps the task running; recreate still fails it.
- `reference_is_current` after regenerating the reference.

**Frontend (Vitest):**

- Reset on `claude_session_id` change; switcher lists and loads earlier
  conversations read-only; composer disabled when viewing an old one.
- Event/turn merge ordering; each inline event row.
- Context meter: 0 after clear, dimmed when stale.
- `session:event_added` prepends to the timeline.
- Phase 3: jump-to-latest / new-pill behaviour, find, copy.

**Manual (one host, provisioned):** start a session, prompt twice, `/clear`,
prompt, `/compact`, `/resume` to the first; check the switcher, the meter and
the timeline after each step. Repeat with two sessions in the same cwd.

## Rollout

1. Ship phase 1; the hub picks up the new hooks on the next `provision_hosts`.
   The local host auto-installs on app start as today.
2. Phases 2 and 3 are frontend plus the parser; they degrade gracefully when a
   hub is on phase 1 without the parser changes (unknown item kinds render as
   plain text).

## Open questions

- Whether `PostCompact` exists on every CLI version the fleet runs. If a host's
  CLI lacks it, `SessionStart(compact)` covers the same transition; the dedupe
  makes installing both safe.
- The exact 1M-context model list for `context_window_for`; unknown models
  default to 200k and the value is corrected if a usage line ever exceeds it
  (window = max(window, 1 000 000) when tokens > 200k).
