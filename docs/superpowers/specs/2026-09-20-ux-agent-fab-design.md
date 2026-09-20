# The UX agent: one button, one operator, the whole fleet

**Date:** 2026-09-20
**Status:** Approved design, awaiting implementation plan
**Builds on:** the control API (`docs/control-api.md`), the confirmation guard
(`crates/fleet-core/src/mcp/guard.rs`) and client tokens
(`crates/fleet-core/src/store/clients.rs`, migration 032)
**Relates to:** `docs/superpowers/specs/2026-09-18-fleet-mobile-design.md` —
the phone client that inherits this work without changing for it
**Slice:** 1 of 2. Slice 2 is the phone, which is mostly `fleet-mobile`'s
existing plan plus one hard security decision (below).

## Goal

A floating button in every view of the desktop app opens an agent that drives
the fleet by talking to the same MCP control API an external assistant would
use. You ask in a sentence; the app repaints because the thing actually
happened.

## Decisions already made

- **One operator per fleet, not one per device.** The agent is a single
  long-lived session. A conversation begun on the desktop continues on the
  phone because it is literally the same conversation. Nothing synchronises.
- **The operator is a first-class fleet session**, tmux-backed (`kind:
  "work"`), visible in the sidebar, attachable, restartable, and already
  covered by host-reboot survival.
- **Its own client token**, named `ux-agent`, mode `full`, minted through
  `insert_client_token`. Never the master token — so fleet admin is closed at
  the auth layer, not by convention, and one `revoke` cuts it off.
- **Destructive calls always confirm**, through the existing
  `McpConfirmDialog` / `E_CONFIRM_REQUIRED` path, regardless of the global
  `mcp.confirm_destructive` toggle.
- **The context chip is visible and removable.** The composer shows what it is
  about to send (`blue-sirius · mefistos · feature/x`); one click drops it.
  No invisible prefixes.
- **FAB plus a keyboard twin (⌘J), one panel.** The button is the discoverable
  affordance and the thing that carries over to a phone; the shortcut is for
  hands already on the keyboard.
- **No new MCP tools.** The agent uses the 74 that exist.

## Non-goals

- Writing code. The operator's working directory is not a repository. When
  work needs doing, it spawns a session that does it.
- A second agent, per project or per host. One operator, no registry.
- Voice-specific code. See *The phone and voice*.
- A confirmation channel for paired clients. Slice 2 decides that; this slice
  must only avoid foreclosing it.

## Where the agent lives

It runs on `local` in desktop mode and on the hub's host in hub mode — always
where the truth about the fleet is.

Its working directory is `~/.claude-fleet/operator/`: not a git repository,
not one of your projects. It holds `CLAUDE.md` (who the operator is, what it
may do, that destructive actions are proposed and confirmed rather than
assumed) and `.claude/settings.json` (the fleet MCP endpoint and its token).

`new_session` requires a `project_id`, so the operator gets an auto-created
project row — `fleet-operator → ~/.claude-fleet/operator` — flagged `system`
and hidden from the project picker. This reuses the session lifecycle
unchanged; the alternative, a new `kind: "operator"`, would add a word to a
vocabulary every filter in the app must then respect.

**Birth is lazy.** The session is created on the first FAB press, not at app
start. An unopened agent costs nothing.

**It may not act on itself.** A guard refuses `kill_session`,
`safe_kill_session`, `move_session` and `restart_session` when the target is
the operator, and excludes the operator from `broadcast_prompt`'s targets.
Without the first, "tidy up the zombie sessions" ends the conversation saying
it. Without the second, the agent prompts itself in a loop that the existing
broadcast rate limiter makes slow enough to look mysterious.

## Components

**Backend**

| File | What it does |
|---|---|
| `crates/fleet-core/migrations/038_project_system_flag.sql` + a row in `schema.rs::MIGRATIONS` | `system BOOLEAN NOT NULL DEFAULT 0` on `projects`; `ProjectRow.system`. The stale-row sweep in `service/projects.rs` skips system rows, for the same reason it already skips `adopted`. |
| `crates/fleet-core/src/service/operator.rs` | The only new service module, and the only place that knows the operator is special: `ensure_operator`, `operator_session`, `operator_token`, `refuse_if_operator`. |
| session-addressed ops | Call `refuse_if_operator`. A few lines, not a new layer. |
| `src-tauri/src/commands/operator.rs` | `ensure_operator` and `operator_status`, both `Routed`. Each needs a `backend/verdicts.rs` row, a `route` by command name, a non-default row in `tests_routing.rs`, a `generate_handler!` entry, then `REGEN_HUB_VERDICTS=1` and `REGEN_DOCS=1`. |

**Frontend**

| File | What it does |
|---|---|
| `src/lib/operator.ts` | Store: the agent's row, panel open state, the open request. Follows `app_views.ts` so the FAB does not prop-drill through `App.svelte`. |
| `src/lib/agent_context.ts` | Pure `appState → { chipLabel, prefix }`. No DOM, tested like `quick_switcher.ts`. The rules will accumulate here. |
| `src/lib/AgentFab.svelte` | The button. Mounted once in `App.svelte` beside `<HintLayer />` and `<McpConfirmDialog />`, which is what makes it present in every view. |
| `src/lib/AgentPanel.svelte` | The sheet: a compact `ConversationPanel`, a `PromptComposer`, the chip. |

Plus a ⌘J binding onto the same store and one `HintId` in `hints.ts` so the
button gets found once.

**Boundaries.** `operator.ts` does not know how a prompt is sent; it calls
`send_prompt`. `AgentPanel` does not know how the agent was born; it asks the
store whether it is ready. `agent_context.ts` does not know there is an agent;
it makes a string. `service/operator.rs` is the single thing to delete if this
feature is ever removed.

## Data flow

Take one sentence: *"kill blue-sirius and start a fresh one on mefistos"*.

1. **Press.** `AgentFab` writes to `operator.ts`; the panel opens and calls
   `ensure_operator`. The first time ever, that means project row, directory,
   `CLAUDE.md`, `settings.json`, token, `new_session`, and the panel shows the
   agent waking. Every later time it is a row read.
2. **Send.** The composer joins the chip's prefix to your text and calls
   `send_prompt`. Delivery is tmux: `send-keys -l <body>`, a 0.15 s settle,
   `send-keys Enter` (`service/sessions/prompt.rs`). The agent receives it
   exactly as if you had typed into its window, because you have.
3. **The agent acts**, calling `kill_session` and `new_session` over HTTP with
   its client token. Every call passes `TOOL_POLICIES`: `Access::Client`
   proceeds, fleet admin is refused before the service layer is reached.
4. **Confirmation.** `kill_session` carries `confirm: true`, so the backend
   answers `E_CONFIRM_REQUIRED` and emits `mcp:confirm-required`.
   `McpConfirmDialog`, already mounted, shows it. You approve, `mcp_confirm`
   releases the nonce, the original call completes, the agent gets its result.
5. **The reply** appears in the panel: `ConversationPanel` polls
   `session_conversation` and receives live timeline events from the hooks.
6. **The sidebar repaints itself.** `blue-sirius` disappears and the new
   session appears through the ordinary row-event bus — not because the agent
   reported it, but because it happened. The agent never has to narrate its
   own work.

Any prompt the operator relays to a work session is prefixed by
`mark_untrusted`, naming the agent as its origin, and `raw: true` is the
master token's alone — so the operator cannot strip the marker, and sessions
always know an instruction came from an agent rather than from you.

## Error handling

**The confirmation window is currently wrong, and this slice fixes it.**
`CONFIRM_TTL` is 600 s (`guard.rs`) while `kill_session` is
`Deadline::Lifecycle`, capped at 300 s (`tools/support.rs`). A nonce outlives
the call waiting on it: approve after six minutes and the agent has been
holding an error for five. Cut `CONFIRM_TTL` to 240 s rather than lengthening
deadlines, and assert the invariant — *a nonce never outlives the call waiting
on it* — with a test that walks `TOOL_POLICIES`, takes the shortest deadline
cap among `confirm: true` rows, and compares. The cost is four minutes to
approve instead of ten. What it buys is that "I confirmed and it failed
anyway" stops existing.

**The agent is dead, or was never born.** `ensure_operator` finds a row with
`lost_at`, or none. The panel says so and offers `restart_session`. It is
never resurrected silently: if it died mid-sentence you want to know, not to
be handed a fresh empty chat.

**The control API is off.** An agent with no tools is a chatbot. The FAB is
disabled with an explanation and a single button to enable it — not a panel in
which the agent apologises. Note the asymmetry: `mcp_status` is `LocalOnly`,
and on a hub the API is always on, so this check has a different shape in hub
mode and must not be carried over blindly.

**The token was revoked.** Revocation is deliberate, so nothing re-mints
itself. `operator_status` compares the stored token against
`active_client_tokens`; the panel says the agent has lost access and offers a
button. One click, but yours.

**Context fills.** The operator is long-lived and will compact. The
`PreCompact` / `PostCompact` hooks already track it (migration 037), so the
panel can show it and needs a visible "new conversation" mapped onto the
existing conversation switch, not a new concept.

**Two clients at once.** The desktop and the phone write into the same tmux
REPL; two interleaved literal pastes make one mangled prompt. For this slice
the honest answer is enough: while `claude_status == working` the composer
refuses to send and says the agent is busy. Real queueing belongs to the slice
where the problem actually arises.

**The agent is stuck.** The `stuck_kind` vocabulary (`auth_menu`,
`trust_prompt`, `press_enter`, …) already exists and the panel surfaces it
directly, because from the outside a stuck agent looks exactly like a slow
one. Being tmux-backed, the last resort is opening its terminal and looking.

## Testing

**The stance first, because it decides the rest:** we do not test whether the
model correctly understands "kill this one". That is model behaviour. This
feature is safe because destructive calls must be confirmed and fleet admin is
closed at the auth layer — not because the agent is reliable. **We test the
guard rails, not the agent.**

**Rust (`cargo test --workspace`)**

- `ensure_operator` is idempotent — twice over gives one project row, one
  session, one token. The most valuable test here, because it runs on every
  press of the button.
- `refuse_if_operator`, as a table: `kill_session`, `safe_kill_session`,
  `move_session`, `restart_session` aimed at the operator return
  `E_FORBIDDEN`; aimed elsewhere they pass.
- `broadcast_prompt` never has the operator among its targets.
- The confirmation invariant above, walking `TOOL_POLICIES` in the manner of
  `every_router_tool_has_exactly_one_policy_row` — so that whoever later adds
  a confirmed tool to the `Quick` class is told why it cannot be.
- Migration 038: `system` arrives defaulted to 0, existing rows untouched, and
  the sweep in `service/projects.rs` leaves system rows alone.
- `backend/tests_routing.rs`: both new commands are `Routed`, each with a
  non-default args row, since the hub client serialises the whole struct.

**Frontend (`npx vitest run`)**

- `agent_context.test.ts` — the pure function over four states: terminal with
  a session selected, Hosts view, Files view, nothing selected.
- `operator.test.ts` — store transitions: unborn → waking → ready → lost.
- `AgentPanel.test.ts` — surfaces `stuck_kind`; refuses to send while
  `claude_status == working`; offers restart when lost.
- `AgentFab.test.ts` — disabled with an explanation when MCP is off; ⌘J
  toggles.

**Regenerations, not tests:** `REGEN_HUB_VERDICTS=1` and `REGEN_DOCS=1` — the
second because `control-api-reference.md` lists every frontend command, not
only MCP tools. CI checks both.

**Manual, once, at the end:** a cold app, the first press, "list the
sessions", then a confirmed kill. Whether the agent is born at all and whether
the dialog really appears is the one thing no unit test covers.

Run the suite whole via `scripts/ci-local.sh`, never filtered per task and
never through `| tail`, or a red test hides. `pnpm install --frozen-lockfile`
first, or missing `@tauri-apps/plugin-*` imports will look like a code error.

## The phone and voice

The hub side is finished and the client is a separate project:
`fleet-mobile` (`docs/superpowers/specs/2026-09-18-fleet-mobile-design.md`) is
an approved Kotlin Multiplatform app in its own repository, explicitly making
no change to `claude-fleet`. It already plans a conversation view of
structured turns and a prompt box.

**Which is the whole point of one operator per fleet: if the agent is an
ordinary session, `fleet-mobile` steers it with nothing added.** It appears in
the session list by itself.

The single obligation this slice carries is that **no FAB action may go
through a `LocalOnly` command**. Today that holds — `send_prompt`,
`session_conversation`, `new_session` and `session_history` are all `routed` —
and the two new commands must be `Routed` too. That is the entire price of
keeping the door open.

**Voice needs no code, and that is the recommendation.** The composer is a
text field and the phone's own keyboard has a microphone. Native dictation
works on both platforms, beats anything embedded here, costs nothing per
request and sends audio nowhere. If push-to-talk is wanted later it goes
beside the composer without touching anything else.

## Open question for slice 2

`McpConfirmDialog.svelte` records the existing trust boundary: *"any script
running in this webview could auto-approve a nonce… the desktop is the trusted
party here, by design. Do not expose `mcp_confirm` to anything less trusted
than this window."*

A phone **is** something less trusted — a paired client, exactly the case that
comment warns about. So "show the dialog on the phone too" is not a UI detail
but a breach of a boundary someone deliberately drew. Slice 2 chooses between
refusing destructive confirmation from a phone (queue it until you are at the
desktop: annoying, safe, no work) and a confirmation channel for paired
clients (comfortable, and needs thinking through properly). This slice only
has to avoid wiring the desktop into `service/operator.rs` as the sole
possible confirming surface.
