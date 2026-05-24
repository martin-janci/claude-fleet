# Onboarding & Setup Flow — Design

**Date:** 2026-05-24
**Status:** Approved (brainstorm), pending implementation plan
**Scope:** Phase 1 of a larger "new-user experience" effort. This spec covers
**only** the guided first-run setup flow. Tutorial tooltips, the app-wide polish
pass, and documentation/README updates are explicitly **out of scope** here and
will each be their own spec → plan → implementation cycle.

## Problem

claude-fleet has no onboarding. A new user lands in an empty app and must
already know to: have `claude` and `tmux` installed, keep hosts in
`~/.ssh/config`, scan `~/projects/github.com`, and understand that adding a host
silently spins up a reverse SSH tunnel for the MCP control server. The reverse
tunnels (`service/tunnel.rs`) run automatically but are completely invisible.

We want a **non-blocking, resumable guided setup** that walks a new user from a
fresh install to a running session, and that **surfaces the existing MCP
tunnels** (their up/down/retrying state) rather than building new tunnel
mechanics.

## Shape (decided during brainstorm)

A **one-time welcome modal** plus a **docked "Get started" checklist card** at
the top of the sidebar scroller. Non-blocking (the app stays fully usable),
resumable, and dismissible. Chosen over a full-screen takeover wizard or a modal
wizard because claude-fleet is a power-user tool and setup naturally happens in
fits and starts (add a host now, provision later).

**Instrumentation depth (decided): Hybrid.** Derive step state from existing
stores; add two small read-only backend commands (`check_local_prereqs`,
`tunnel_status`). Provisioning stays a blocking call with a spinner + per-host
result line. No new event-bus types. (A fully-instrumented streaming version was
considered and deferred.)

## Architecture

The checklist is a **derived view over real fleet state**, not a stored
state machine. Each step's done/pending status is computed live on every render
from: the `hosts`, `projects`, and `sessions` stores; `mcp_status()`; and
snapshots from the two new commands. This keeps the checklist truthful across
reinstalls, manual changes, and background reconciliation — there is no
checklist state to drift out of sync.

Only **two booleans** are persisted, via the existing `prefs.ts` localStorage
helpers (`readPref`/`writePref`, prefix `cf:pref:`):

- `onboarding-welcomed` — has the one-time welcome modal been shown.
- `onboarding-dismissed` — has the user dismissed the card.

Neither step results nor prereq/tunnel snapshots are persisted; they are
recomputed on demand.

### New frontend units

- **`src/lib/onboarding.ts`**
  - Two persisted flag stores (`onboardingWelcomed`, `onboardingDismissed`),
    following the `showBgAgents` auto-subscribe pattern in `sessions.ts`.
  - A **pure** function `deriveSteps(inputs): OnboardingStep[]` mapping
    `{ hosts, projects, sessions, mcp, prereqs, tunnels }` → an ordered list of
    steps each with `{ id, label, sublabel?, status, optional, badge? }` where
    `status ∈ 'done' | 'active' | 'pending'`. Pure = unit-testable in isolation.
- **`src/lib/WelcomeDialog.svelte`** — one-time intro modal, styled to match the
  existing `SettingsDialog`/`AddHostPicker` dialog conventions (fixed backdrop
  `rgba(0,0,0,0.4)`, centered panel). Buttons: "Let's set up →" (reveals/keeps
  the card) and "Skip for now" (sets `welcomed`, leaves card visible until
  dismissed).
- **`src/lib/OnboardingCard.svelte`** — the docked card. Renders `deriveSteps`
  output; each row is clickable and triggers that step's action. Owns the inline
  async UI (spinners, error text, retry) for prereq check, provisioning, and
  project scan.

### Touch points (existing files)

- **`src/lib/Sidebar.svelte`** — mount `OnboardingCard` at the top of the
  scroller (~line 769, before the project-tree conditional), visible while
  `!onboardingDismissed`.
- **`src/App.svelte`** — after bootstrap (mount, ~line 93-119), show
  `WelcomeDialog` once when `!onboardingWelcomed && visibleHosts === 0 &&
  workSessions === 0`.
- **`src/lib/SettingsDialog.svelte`** — add a "Replay setup guide" button that
  sets `onboardingDismissed = false` (and optionally `onboardingWelcomed =
  false`) so the flow can be reopened.

## The steps

Order and done-conditions (all auto-derived unless noted):

| # | Step | `done` when | Click action |
|---|------|-------------|--------------|
| — | **Welcome** (modal, not a row) | `onboarding-welcomed` true | "Let's set up" → dismiss modal, card remains |
| 1 | **Local prerequisites** | `claude_ok && tmux_ok && projects_readable` | run `check_local_prereqs`; render results with install hints + doc links for anything missing. **Warn-only — never blocks.** |
| 2 | **Add a host** | ≥1 non-hidden host (`$hosts.filter(h => !h.hidden).length > 0`) | open `AddHostPicker` |
| 3 | **Provision & tunnels** | host `provisioned` AND its tunnel `state === 'up'`; if MCP off, treated as done-able with an honest "starts with Control API" badge | run `provision_hosts` (spinner + per-host `HostProvisionResult` line); show tunnel badge per host |
| 4 | **Pick projects** | `$projects.length > 0` | run `refresh_projects`; show discovered count |
| 5 | **Enable Control API** *(optional)* | `mcp_status().enabled` | enable via `mcp_configure({ enabled: true })`; show port + masked token with copy; tunnel badges go live |
| 6 | **Create first session** | ≥1 **work** session exists | open `NewSessionDialog` — the finish line |

"Work session" excludes background (`bg:` / `kind: "background"`) sessions, so
spinning up a background agent doesn't falsely complete onboarding.

### Tunnel ↔ MCP dependency (made honest, not hidden)

Reverse tunnels exist only to expose the app's MCP server to remote hosts, so a
tunnel is only meaningful once the Control API is enabled. The card reflects this
truthfully via the tunnel badge:

- MCP disabled → amber badge **"tunnel: starts with Control API"** with a link to
  step 5 / `docs/control-api.md`.
- MCP enabled, task alive → green **"tunnel: up"**.
- MCP enabled, task dead/looping → amber **"tunnel: down — retrying"**.

The implementation plan must confirm in code exactly when `TunnelSupervisor::
ensure` is invoked relative to MCP enable; the `tunnel_status` mapping below is
written to that contract.

## New backend commands

Both are **Tauri-only commands** (registered in `generate_handler!`), **not**
`#[tool(...)]` MCP tools — so neither triggers `control-api-reference.md`
regeneration. Both are read-only.

### `check_local_prereqs() -> LocalPrereqs`

```rust
struct LocalPrereqs {
    claude_ok: bool,
    claude_version: Option<String>,   // parsed from `claude --version`
    tmux_ok: bool,
    tmux_version: Option<String>,     // parsed from `tmux -V`
    projects_path: String,            // from service::projects::projects_base()
    projects_readable: bool,          // path exists and is a readable dir
    projects_count: u32,              // top-level entries, 0 if unreadable
}
```

- Lives in `commands/onboarding.rs` (thin) + `service/onboarding.rs` (logic),
  following the commands/→service/ split.
- Runs the local binaries via `tokio::process` with a short timeout; a missing
  binary yields `*_ok = false`, never an error.

### `tunnel_status() -> Vec<TunnelStatusRow>`

```rust
enum TunnelState { Up, Down, Starting, NotStarted }

struct TunnelStatusRow {
    host_alias: String,
    state: TunnelState,   // serialized lowercase
}
```

- Adds a `snapshot()` method to `TunnelSupervisor` (`service/tunnel.rs`) that
  reports, per known host, whether a supervised task exists and
  `is_finished()`. `NotStarted` = no task for that host (e.g. MCP disabled).
- Returns one row per non-hidden host so the card can pair status to hosts.

## Error handling

Nothing in the flow blocks the app or throws an unhandled error to the user.

- **Prereq missing:** inline warning on the step with an install hint
  (`brew install tmux`, link to Claude Code install docs) and the resolved
  projects path. The step stays incomplete but the user can proceed.
- **Provision failure:** show `HostProvisionResult.detail` inline with a
  **Retry** button (re-invokes `provision_hosts`).
- **Tunnel down/retrying:** amber badge + link to `docs/control-api.md`.
- **MCP bind error:** surface the existing `McpStatus.bind_error` text.
- All async actions unwrap the standard `Result`/`IpcError` (`src/lib/result.ts`)
  and render the `E_*`-coded message inline; failures never dismiss the card.

## Testing

- **Frontend (Vitest):**
  - `onboarding.ts` — `deriveSteps` over representative input combinations:
    nothing set; host added; provisioned + tunnel up vs. MCP-off badge; projects
    scanned; all-complete; optional-skipped. Assert correct `status`, `optional`,
    and `badge` per step.
  - Flag persistence: `onboardingWelcomed`/`onboardingDismissed` round-trip
    through `prefs.ts`.
  - `OnboardingCard` render states (done / active+spinner / pending / error).
  - (Be aware of the pre-existing `localStorage is undefined` test-env failures
    noted in CLAUDE.md — verify new tests against `main` behavior.)
- **Backend (cargo test):**
  - `tunnel_status` mapping: supervisor with a live task → `Up`; finished task →
    `Down`; unknown host → `NotStarted`.
  - `check_local_prereqs` version-string parsing for `claude`/`tmux` outputs.
  - (Headless cargo build needs Tauri system libs per CLAUDE.md; note the
    environment caveat if it can't run here.)
- **Manual:** fresh-state walkthrough — welcome → each step → "all set" →
  dismiss → reopen from Settings.

## Out of scope (future phases)

- Reusable Tooltip/coachmark component and a guided product tour.
- App-wide visual polish pass.
- `docs/` getting-started guide and README rewrite.
- Streaming per-step provisioning progress via the event bus.
- A settable projects scan path UI (path is env/default only today).
