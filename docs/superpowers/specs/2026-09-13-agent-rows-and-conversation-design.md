# Agent rows outside tmux, and the Conversation tab

Date: 2026-09-13. Status: approved in conversation (option B).

## Problem

Verified on the author's Mac (Claude Code 2.1.235) and the live fleet store:

1. **Misclassification.** `reconcile_bg_agents` turns every `claude agents
   --json` row that no tmux session claimed into a synthetic `kind='bg'` row
   (`bg:<uuid>`). The CLI's `kind` field is parsed but dropped. On `local`,
   8 of 11 "background" rows are interactive Claude Desktop / terminal
   sessions, including the session that wrote this spec. 22 bg rows fleet-wide.
2. **The log never works.** `BgSessionPanel` and the row's 📋 peek run
   `claude logs <full uuid>`. The subcommand only accepts the short job id
   (`44366faf`, the agents row's `id`), so it always answers "No job
   matching", which `claude_logs` rewrites to "No background logs — this is
   an interactive session". With the short id it still fails when the daemon
   is gone (`connect ENOENT …/control.sock`), and it can never work for an
   interactive session.
3. **Kill on a bg row is a no-op.** `claude stop <full uuid>` → "No job
   matching" → treated as "already stopped" → the next reconcile re-imports
   the agent, which `claude agents` still lists.
4. **Dead agents pose as blocked.** The three real bg agents (May–July) sit in
   `state: blocked` waiting for a reply or a `/login`, but no daemon runs
   them. They inflate blocked/attention signals indefinitely.

The transcript JSONL (`~/.claude/projects/<enc cwd>/<sessionId>.jsonl`) is
readable for every one of these rows and survives the process; the backend
already reads it (`service/transcript.rs`, MCP `session_transcript`), but the
UI never does and the parser discards the user's prompts.

## Decisions (from the conversation)

- Interactive sessions outside tmux: a collapsed **"Outside fleet (N)"**
  sidebar group, read-only.
- Inactive bg agents: shown as stopped (grey), with **Remove from list**;
  fleet remembers the removal.
- The log is replaced by a **Conversation** tab next to Terminal / Files for
  every session that has a `claude_session_id`; 📋 goes away.

## 1. Classification (backend)

`ClaudeAgentRow` gains:

- `kind: AgentKind` — `Interactive` for `"interactive"`, `Background` for
  `"background"` **and for a missing or unknown value** (older CLIs only
  listed bg agents; keeps today's behaviour on hosts running them);
- `job_id: Option<String>` — the row's `id` (the short id `claude stop` /
  `claude attach` take); validated `^[0-9a-f]{8,36}$` before any use;
- `started_at: Option<i64>` — `startedAt` (ms) converted to unix seconds.

`reconcile_bg_agents` (renamed `reconcile_agent_rows`) upserts each unmatched
agent with `kind = 'external'` for `Interactive`, `'bg'` for `Background`.
The sentinel `tmux_name` stays `bg:<sessionId>` for both (every "no pane"
guard already keys on that prefix). `upsert_bg_session` takes the kind as a
parameter and writes it on insert and on conflict, so the 8 misfiled rows
flip to `external` on the first pass after upgrade. `ghost_and_clean_bg_sessions`
covers `kind IN ('bg','external')`.

Matching tmux sessions to agents (`find_for_session`) is unchanged.

## 2. Inactive bg agents

A bg agent is **inactive** when its CLI status is not `working` and its
transcript has not been modified for `AGENT_INACTIVE_SECS = 86_400`. With no
transcript found, `started_at` stands in for the mtime; with neither, it is
active (never guess dead).

The mtimes come from one extra host call per reconcile pass, made only when
the pass saw at least one `Background` agent: a script, built with
`crate::shell::quote` for every id, printing `<sessionId>\t<mtime>` for each
`"$HOME"/.claude/projects/*/<sessionId>.jsonl` via `date -r "$f" +%s`
(identical on GNU and BSD). Ids are validated with
`validate::claude_session_id` first; an invalid id is skipped. The call runs
off the store lock like the other probes, bounded by the existing
`run_bounded` wall clock; a failed call leaves every agent active.

An inactive agent is upserted with `claude_status = 'stopped'` (existing
vocabulary; `idle_since` follows the existing rule). Nothing else changes in
the status vocabulary, the MCP descriptions or the generated reference.

## 3. Remove from list

Migration `029_dismissed_agents.sql`:

```sql
CREATE TABLE IF NOT EXISTS dismissed_agents (
  host_alias TEXT NOT NULL,
  claude_session_id TEXT NOT NULL,
  dismissed_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, claude_session_id)
);
```

- `Store::dismiss_agent(host, claude_session_id, now)` upserts the row, then
  deletes the session row and emits `session:removed`.
- `Store::dismissed_agents(host) -> HashMap<String, i64>`.
- Reconcile skips an agent whose dismissal is **at or after** its transcript
  mtime (or `started_at`). Activity after the dismissal (a newer mtime)
  deletes the dismissal and the agent reappears. An agent with no known time
  stays dismissed.
- Command `dismiss_agent_session { session_id }` (Tauri; frontend-only, not an
  MCP tool) accepts only a `kind='bg'` row whose `claude_status` is not
  `working`; otherwise `E_INVALID_STATE`. `external` rows are not dismissable
  (they leave the list when their process ends).

## 4. Stop, and launching

- `kill_session` on a `bg:` row: `kind='external'` → `E_INVALID_STATE` ("this
  Claude session runs outside fleet; close it where it runs"). `kind='bg'` →
  look the agent up in a fresh `list_claude_agents` for that host by
  `sessionId`, and run `claude stop <job_id>`. Agent absent from the listing →
  treat as already gone (reconcile prunes the row). Agent present with no
  valid `job_id` → `E_INVALID_STATE` suggesting Remove from list.
  `stop_script` validates the short-id shape instead of a full UUID.
- `new_bg_session_tracked`: after `claude --bg --name <name>` succeeds, find
  the agent by `name` in `list_claude_agents` (up to 3 tries, 1 s apart)
  instead of trusting `parse_session_id_from_bg_output`. The parsed id stays
  as the first choice when present. Neither found → today's warning, and the
  row appears on the next reconcile.

## 5. Sidebar and roll-ups (frontend)

- `sessions.ts`: `hasNoPane(s) = s.kind === 'bg' || s.kind === 'external'`,
  used everywhere the code now tests `kind === 'bg'` for "no PTY" (App tabs,
  TerminalView, SessionDetails Repair, OnboardingCard, hints, workSessionCount).
- `sidebar_index.ts`: `external` rows are excluded from
  `buildSessionsByProject` and from the orphan list; a new
  `buildOutsideFleet(sessions, hostFilter)` returns them sorted by
  `last_activity_at` desc. The host filter applies; `showBgAgents` does not.
- `Sidebar.svelte`: after "Other sessions", a section header button
  `Outside fleet (N)` (`data-testid="outside-fleet"`), collapsed by default,
  state persisted with `writePref('outside-fleet-open', …)`. Rows render with
  `SessionRowItem` in a `readOnly` mode: status chip and name only; no
  restart / label / kill / rename actions; selecting still works.
- A bg row whose `claude_status` is `stopped` shows a grey "inactive" chip and
  a **Remove from list** action (row menu and SessionDetails) calling
  `dismissAgentSession`; the row disappears via the `session:removed` event.
- `attention.ts`: `external` rows contribute nothing to severity, attention
  counts or stuck alerts. `service/health.rs` (`fleet_health`) skips
  `kind='external'` rows in its counts.

## 6. The Conversation tab

**Backend.** `service/transcript.rs` gains a structured parser beside the
text one:

```rust
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ConvTurn {
    pub prompt: Option<String>,   // the human prompt that opened the turn
    pub at: Option<String>,       // the prompt's (else first entry's) ISO timestamp
    pub items: Vec<ConvItem>,
}
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConvItem {
    Text { text: String },
    Tool { summary: String },     // same one-liner as today's [tool_use] line
}
pub fn parse_conversation(jsonl: &str) -> Vec<ConvTurn>;
```

`parse_turns` is reimplemented on top of `parse_conversation` (join items,
tool lines prefixed `[tool_use] `, turns with no items dropped), so the MCP
text output is byte-identical: the existing `parse_turns` tests stay as they
are and must still pass. Sidechain entries, thinking blocks and tool results
stay excluded. A prompt whose content is a list of text blocks joins them.

`fetch_conversation(args, ssh) -> Conversation { turns: Vec<ConvTurn>,
truncated: bool }` reuses `read_script` / `run_shell`; keeps the last
`CONV_TURNS = 10` turns and trims from the oldest item until the rendered
characters fit `CONV_MAX_CHARS = 64_000` (`truncated = true` when anything
was dropped). The cwd / stored-path resolution that `transcript_for` does in
`mcp/tools/support.rs` moves into a shared
`transcript::resolve_args(store, row)` both callers use.

Command `session_conversation { session_id } -> Conversation` (Tauri;
regenerates the control-API reference). Errors as `fetch_transcript`:
`E_INVALID_STATE` (no `claude_session_id`), `E_NO_TRANSCRIPT`, transport codes.

**Frontend.** `src/lib/conversation.ts` (IPC wrapper + pure helpers) and
`ConversationPanel.svelte`:

- A third tab `Conversation` (`data-testid="tab-conversation"`) between
  Files and Hosts, enabled when the selected session has a
  `claude_session_id`, else disabled with the title "No Claude session id yet".
- Selecting a `bg` / `external` row opens Conversation by default; Terminal
  and Files are disabled for them with the title "Runs outside tmux — no
  terminal". The terminal is not mounted for those rows (as today).
- Conversation reuses the Files overlay mechanism, so a tmux session's PTY
  stays mounted underneath; Files, Hosts and Conversation are mutually
  exclusive.
- Rendering: each turn shows its prompt (quoted block, relative time), then
  text items as `pre-wrap` prose and tool items as single muted monospace
  lines. No markdown rendering. `truncated` shows "Older turns not shown".
- Refresh: fetch on open and on session change; poll every 5 s only while the
  tab is visible and `document.visibilityState === 'visible'`; replace state
  only when the result differs; a stale response for a previous session is
  dropped (sequence counter, as `Timeline.svelte`). Scroll stays pinned to
  the bottom unless the user has scrolled up more than 40 px.
- States: loading; `E_NO_TRANSCRIPT` → "No conversation yet"; no id →
  "No Claude session id yet"; any other error → the message plus Retry,
  keeping the last good turns visible.

## 7. Removals

- Frontend: `BgSessionPanel.svelte`, `PeekPanel.svelte`, the 📋 button and
  `peekState` in `Sidebar.svelte`/`SessionRowItem.svelte`, `peekSession` in
  `sessions.ts`, and their tests.
- Backend: the Tauri `peek_session` command, `bg_sessions::peek_session`,
  `claude_cli::claude_logs`, `logs_script`, `NO_BG_LOGS_MSG`.
- MCP `peek_session` stays for compatibility: it resolves the row as today
  and returns the last turn via `transcript_for`; its description says it is
  deprecated in favour of `session_transcript`. Regenerate the reference.

## Tests

Rust:
- `claude_agents`: kind parsing (interactive / background / missing /
  unknown → Background), `job_id` shape validation, `started_at` ms → s.
- reconcile: an unmatched interactive agent lands as `external`, a background
  one as `bg`; a misfiled `bg` row flips to `external`; cleanup ghosts both
  kinds.
- inactive rule: non-working + old mtime → `stopped`; `working` + old mtime →
  unchanged; no mtime → `started_at`; neither → active; mtime script quotes
  ids and skips invalid ones.
- dismissal: dismissed agent skipped; newer mtime revives it and clears the
  dismissal; command refuses `external` and `working` rows.
- kill: `external` refused; bg uses `job_id`; absent agent → no error.
- launch lookup by name when the output has no id (FakeSsh).
- `parse_conversation`: prompts captured, tool one-liners, sidechain and
  thinking excluded, text-block prompts joined; `parse_turns` output unchanged
  (existing tests); truncation drops the oldest items and sets `truncated`.
- migration 029 idempotent.

Frontend:
- `buildOutsideFleet` and exclusion from project/orphan lists; host filter.
- Sidebar: group collapsed by default, remembers open; read-only rows have
  no actions.
- App: Conversation tab enabled/disabled rules; bg/external selection opens
  Conversation and disables Terminal/Files; tabs mutually exclusive.
- ConversationPanel: renders prompt/text/tool items; empty and error states;
  polls only while visible; drops stale responses; no re-render on identical
  data.
- attention: external rows ignored.

## Out of scope

- Timeline events for bg / external rows (Conversation covers the need).
- Loading older turns.
- ~~Markdown rendering, sending prompts from the tab.~~ Both landed later:
  reply text renders as markdown, and a tmux-backed session gets a composer
  under the thread (`send_prompt`, the same path as the Send-prompt dialog;
  Enter sends, Shift+Enter breaks a line). The sent prompt shows as a pending
  turn until a poll brings back a transcript carrying it. bg / external rows
  stay read-only, with a note saying why.
- Live indicator (later still): under the last turn the tab shows a pulsing
  "Working…" row with the REPL's spinner text, "Sent, waiting for Claude…"
  after a composer send, or an amber "Claude is waiting for you in the
  terminal" banner with an Open terminal button when the pane shows a
  dialog. Source: the row's `claude_status` (row events) laid under an
  on-demand `session_activity` probe (one `capture-pane` of the tail run
  through `pane_intel::analyze`, polled every 2 s only while something is
  live). A quiet session (idle / completed / stopped / failed) re-reads the
  transcript only every 15 s or when its `turn_seq` moves, instead of the
  5 s cadence.
- The tmux-session cwd fallback in `find_for_session` can, in principle,
  bind a tmux row to an interactive session running elsewhere in the same
  directory; unchanged here.
- Remote hosts on old CLIs keep today's behaviour (no `kind` → bg).
