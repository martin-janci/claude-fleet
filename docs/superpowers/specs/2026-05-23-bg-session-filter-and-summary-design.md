# Background-session filter + summary panel

**Date:** 2026-05-23
**Status:** Approved (design)

## Problem

Background ("bg") Claude sessions — `kind === 'bg'` rows synthesized by
`reconcile_bg_agents` — have no tmux pane. Two problems follow:

1. They clutter the sidebar tree alongside interactive sessions with no way to
   hide them.
2. Selecting one mounts `TerminalView`, which calls `pty_open` →
   `tmux attach -t bg:<uuid>`. That fails (no pane), and the error renders in
   the terminal pane (`TerminalView.svelte:728`). There is no useful view of
   what the background agent is doing.

We already have the data to show something better: the session row carries
`claude_status`, `current_activity`, and `pr_url` (kept fresh via row events),
and the existing `peek_session` path returns the agent transcript via
`claude logs <id>`.

## Goals

- A sidebar toggle to show/hide background agents.
- A dedicated summary + logs view for a selected bg session, replacing the
  broken console.
- Reuse existing data paths; no backend changes.

## Non-goals

- No new backend command or server-side aggregation.
- No change to how bg sessions are created or reconciled.
- No live PTY/streaming for bg sessions.

## Approach

Purely additive frontend work. Rejected alternatives:

- **New backend bg-summary command** aggregating status + logs server-side —
  more code, duplicates data already on the row.
- **Make `pty_open` return logs for bg sessions** — overloads the PTY
  abstraction and fights the `TerminalView` model.

Reusing the existing `peek_session` command and row-event status flow is the
smallest, cleanest change.

## Design

### 1. Sidebar filter toggle

- New persisted store `showBgAgents` (writable, default `true`,
  localStorage-backed) alongside the existing filter stores
  (`hostFilter`, etc.).
- A small toggle in the sidebar filter row ("Background agents: on/off").
- When **off**, both `filteredSessionsByProject` and `orphanSessions`
  (`Sidebar.svelte:169-211`) exclude `kind === 'bg'`. When **on** (default),
  behavior is unchanged.
- bg rows get a small "bg" badge in the sidebar, mirroring the existing
  `review` (`Sidebar.svelte:589`) and `shell` (`:592`) badges.

### 2. Right-pane summary panel (`BgSessionPanel.svelte`)

- In `App.svelte`, the right column renders `BgSessionPanel` when
  `$selectedSession?.kind === 'bg'`, otherwise `TerminalView` as today.
- Because `TerminalView` unmounts for bg sessions, `pty_open` is never called —
  the "no tmux" error path is eliminated, not merely hidden.
- Panel shows:
  - `claude_status` with the status styling used elsewhere in the UI.
  - `current_activity`.
  - `pr_url` as a link when present.
  - Transcript from `peekSession(host_alias, claude_session_id)` (existing
    `claude logs <id>` path).
- If `claude_session_id` is missing → clean placeholder ("background agent — no
  logs available yet"), no fetch attempted.

### 3. Auto-poll

- On select, fetch logs once; then poll every **10s** while the panel is
  mounted. Status / activity / PR already update live via row events, so polling
  only re-fetches the transcript.
- A manual **Refresh** button is also provided.
- The interval is cleared on unmount and on selection change.
- On fetch error, keep the last good transcript and show a small inline error
  rather than blanking the panel.

## Affected files

- `src/lib/sessions.ts` (or `selection.ts`) — `showBgAgents` store.
- `src/lib/Sidebar.svelte` — toggle UI, filter exclusion, bg badge.
- `src/lib/BgSessionPanel.svelte` — new component.
- `src/App.svelte` — conditional right-pane rendering.

No backend (`src-tauri`) changes.

## Testing (Vitest)

- Sidebar filter excludes bg rows when `showBgAgents` is off and includes them
  when on.
- `BgSessionPanel` renders status / activity / logs from a mocked
  `peekSession`.
- Polling starts on mount and stops on unmount (mock timers + `peekSession`).
- Missing-`claude_session_id` placeholder path renders without fetching.
