# Background work in the Conversations tab — design

- **Date:** 2026-09-20
- **Status:** draft, awaiting review
- **Builds on:** `docs/superpowers/specs/2026-09-18-conversation-events-design.md`
  (the Conversations tab, its switcher, inline events and parser)

## Goal

The Conversations tab shows what a session's background work is doing, in the
language of the work rather than the language of the transcript format. A
`<task-notification>` reads as a finished agent, not as XML. Every background
thing that belongs to a session — the agents and commands it launched inside
its own conversation, and the fleet rows it spawned — is reachable from one
switcher, and picking one replaces the thread with that thing's view.

## Problems today

1. **Notifications render as raw XML.** `parse_conversation` classifies a user
   entry as a slash command (`<command-name>`) or its output
   (`<local-command-stdout>`); everything else falls through to `prompt_text`
   and `ConversationPanel` prints it verbatim
   (`ConversationPanel.svelte:1101`). A `<task-notification>` is therefore a
   turn whose "prompt" is a wall of tags. Across the author's transcripts there
   are 1242 of them.
2. **A launched background agent shows internal metadata as its result.** An
   `Agent` tool call returns `"Async agent launched successfully. … agentId:
   …"` as its `tool_result`. `SUBAGENT_TOOLS` picks that up and
   `SubagentBlock` renders it as the subagent's report. The real report arrives
   later, in the notification, and never reaches the block.
3. **The report is capped for the wrong shape.** `SUBAGENT_RESULT_MAX_CHARS` is
   1 500. A background agent's report is routinely several KB, so what does
   survive is truncated mid-sentence.
4. **Background work has no index.** There is no way to see, from the
   conversation, which agents or commands are still running, and no way to move
   between them. Fleet's own children (`dispatch_task` workers, `new_bg_session`
   rows) are listed in the Details pane, one list per kind, not beside the
   conversation they came from.
5. **`new_bg_session` records no parent.** `sessions.parent_session_id` exists
   (migration 020) and `dispatch_task` sets it on the worker
   (`orchestration.rs:256`), but a background session spawned by an agent has
   no link back to the session that asked for it.

## Non-goals

- Reading a background task's `<output-file>` off the host. The path is shown
  and copyable; the body comes from the transcript. The file lives under
  `/private/tmp/...` and does not survive a host reboot, so a reader would need
  its own "it's gone" state — a separate slice if it is ever wanted.
- Rendering a background agent's own transcript. Background agents run out of
  process; their entries are not in the session's `.jsonl` at all (a scan of
  the author's transcripts found zero `isSidechain` lines beside 1242
  notifications), so there is nothing to parse.
- New hooks, new lifecycle tracking, or per-subagent cost accounting.
- Changing how background work is launched.

## The notification format, as observed

Every `<task-notification>` is a user entry whose text *starts* with the tag.
Tallied over the author's 200 most recent transcripts:

| Field | Count | Notes |
|---|---|---|
| `task-id` | 1217 | Stable identity of the background task |
| `summary` | 1217 | One human line; always present |
| `status` | 1128 | `completed` \| `failed` \| `stopped` \| `killed` |
| `output-file` | 1114 | Path on the session's host |
| `tool-use-id` | 1088 | Joins back to the launching `tool_use` |
| `result` | 708 | The agent's full report |
| `usage`, `note` | ~700 | Model-facing; not shown |
| `event` | 91 | Monitor's streamed line |

By launching tool: `Agent` 486, `Bash` 406, `SendMessage` 172, `Workflow` 14,
`Monitor` 7, and 154 with no `tool-use-id` at all (Monitor events mid-stream).
Where a `tool-use-id` was present it matched a `tool_use` in the same thread in
every case.

`note` says a task-id may notify more than once: an agent that is resumed
notifies again under the same id.

## Backend — parsing

### A new `ConvItem` variant

```rust
/// A `<task-notification>` user entry: a background agent, command, monitor
/// or workflow reporting in. `tool_use_id` joins it to the item that
/// launched it when that item is in the loaded window.
Notification {
    task_id: Option<String>,
    tool_use_id: Option<String>,
    status: Option<String>,      // completed | failed | stopped | killed
    summary: Option<String>,
    result: Option<String>,      // capped at NOTIFICATION_RESULT_MAX_CHARS
    output_file: Option<String>,
    event: Option<String>,
}
```

`note` and `usage` are dropped: the first is an instruction to the model, the
second is accounted for elsewhere.

`NOTIFICATION_RESULT_MAX_CHARS = 20_000`, matching `COMPACT_SUMMARY_MAX_CHARS`
rather than the 1 500 that `SubagentBlock` was sized for. Capping is by `char`,
not by byte.

### Where it is recognised

In the same place as slash commands, under the same rule: only an entry whose
trimmed text *starts* with `<task-notification>` is one. A human prompt that
merely quotes the tag — a pasted transcript — stays a prompt.

### Joining to the launching item

When `tool_use_id` names an item in the loaded window:

- `ConvItem::Subagent { id }` — the block becomes `done`, takes `ended_at` from
  the notification's timestamp, sets `error` when `status` is anything but
  `completed`, and **replaces** its `result` with the notification's. This is
  what removes the `"Async agent launched successfully…"` text.
- `ConvItem::Tool { id }` (`Bash`, `Monitor`, `Workflow`) — the line becomes
  `done` and takes `ended_at`, with `error` set the same way. Its own summary
  is kept: `Bash(command=…)` is the useful text, and the notification's
  sentence already has its own row.

A notification whose `tool_use_id` is absent, or names an item outside the
loaded window, joins nothing and stands alone. This is the documented
degradation if the format ever changes.

### Turn placement

A notification opens a prompt-less turn carrying the `Notification` item — the
same shape `Command` already uses — so whatever the agent does in response is
grouped under it. Consecutive notifications with no assistant output between
them merge into one such turn, so three agents finishing together do not
produce three near-empty turns.

### Wire contract

`ConvItem` is a `session_conversation` wire type, so the hub contract golden
test fails until regenerated:

```bash
REGEN_HUB_CONTRACT=1 cargo test -p fleet-core   # reports FAILED; re-run to verify
```

## Backend — fleet parentage

`NewBgSessionArgs` gains an optional `requester_session_id: Option<i64>`. After
`new_bg_session_tracked` matches the new row, the service calls
`set_parent_session_id(row.id, requester)` — the same thing `dispatch_task`
does for its worker. No migration: the column has existed since migration 020.

The desktop dialog passes `None` (nobody asked for it from inside a session).
An agent driving the control API passes its own session id.

Consequences to regenerate:

- The MCP tool schema changes → `REGEN_DOCS=1 cargo test -p fleet-core
  reference_is_current`. The parameter's doc comment stays one sentence; the
  served tool surface is capped by a test.
- `new_bg_session` is hub-routed and the hub client serialises the whole args
  struct, so the new field needs a **non-default** value in the
  `backend/tests_routing.rs` row or the test cannot see it cross the wire.
- No new Tauri command, so `verdicts.rs` and `hub_verdicts.generated.json` are
  untouched.

## Frontend — the thread

The compact row reuses the existing `.event` row
(`ConversationPanel.svelte:1058`: label · detail · time, tone
info/warn/error), made clickable:

```
✓ Agent "Posúdiť stratégiu testov" finished · 12m
✕ Background command "Watch Docker build CI" failed (exit 1) · 3m
• Monitor "PR #165 CI checks": frontend (ubuntu-24.04): pass · 1m
```

Tone comes from `status`: `completed` → info, `failed` / `killed` → error,
`stopped` → warn, absent → info. A notification with only an `event` (no
status) is a plain info line.

`SubagentBlock` keeps its place in the thread and now renders the real report.
It gains a status badge and an "open" affordance to the same detail view.

## Frontend — the switcher

A `N background` dropdown joins `⌕ Find` and `N turns` in the thread toolbar —
the same idiom as the turn index, so it costs no vertical space. Two groups:

**In this conversation.** Derived from the parsed conversation alone: each
launching item (`Agent`, `Bash`, `Monitor`, `Workflow`) paired with the
notifications that carry its `tool-use-id`. An entry is keyed by its `task_id`
once one has been seen and by the launching `tool_use` id until then; a
launching item with no notification yet is `running`. Running entries sort first, then by
start time descending.

**Fleet children of this session.** `$sessions` where `parent_session_id ===
session.id` (background sessions and dispatched-task workers) and `$tasks`
where `requester_session_id === session.id`. Both stores are already populated;
no new backend call.

The transcript group is scoped to the conversation being viewed — after
`/clear` it is empty, consistent with the rest of the thread and with the
existing conversation switcher. The fleet group is session-wide.

## Frontend — switching

One new piece of panel state, `background: string | null`, alongside today's
`viewing`. When it is set, `thread-area` renders `BackgroundDetail.svelte`
instead of the scroller, and the composer is hidden — the composer writes to
the conversation, not to a finished agent.

| Entry | What opening it does |
|---|---|
| Transcript entry | Detail view: what it was asked to do, status, duration, the report as Markdown, and the `output-file` path with a copy button. A task-id that notified more than once lists its notifications in time order. |
| Fleet task | Same view, filled from `TaskRow`: prompt, result or error, states and timings, and a link to the worker session. |
| Fleet session | `selectSession(row)` — the whole app switches to that session, as the Reviews panel already does (`SessionDetails.svelte:507`). A session has its own transcript, terminal and composer; anything less would be a worse view of it. |

Getting back is the breadcrumb `← Back to conversation`, in the slot the
"Viewing an earlier conversation" banner already occupies.

## Testing

Rust parser tests, each from a shape observed in a real transcript:

- an `Agent` notification joins by `tool-use-id` and replaces
  `"Async agent launched successfully…"` with the report
- a `Bash` notification (no `result`, a `summary` carrying the exit code)
  closes its tool line
- a `Monitor` event with no `tool-use-id` stands alone and joins nothing
- `failed` / `killed` / `stopped` each set `error` and the right tone
- a prompt that merely *contains* `<task-notification>` stays a prompt
- three notifications in a row with no assistant output merge into one
  prompt-less turn
- a `result` over 20 000 chars is cut on a char boundary
- the same `task-id` notifying twice leaves the block on the latest state while
  the detail keeps both

Frontend: list derivation and tone mapping in `conversation.ts`; the dropdown,
switching and return in `ConversationPanel.test.ts`; a new
`BackgroundDetail.test.ts`.

Verification is the whole `scripts/ci-local.sh`, not filtered runs.

## Delivery

| Slice | Scope |
|---|---|
| 1 — Parser | `ConvItem::Notification`, the join, turn placement, contract regen |
| 2 — Thread | Clickable event row, `SubagentBlock` showing the real report |
| 3 — Switcher | Toolbar dropdown, `BackgroundDetail`, switching and return |
| 4 — Fleet parentage | `new_bg_session` requester, docs and routing regen |

Slices 1 and 2 are worth shipping even if 3 and 4 slip: they are what makes the
tab stop printing XML.

## Risks

- **The join key.** The design assumes `<tool-use-id>` is stable. It matched in
  all 1088 observed cases where it was present. If the format changes, a
  notification degrades to a standalone row — never back to raw XML.
- **Window edges.** A notification can arrive for an agent launched before the
  loaded turn window starts. It then joins nothing and shows as a standalone
  row, which is honest; "Load older" brings the pair back together.
