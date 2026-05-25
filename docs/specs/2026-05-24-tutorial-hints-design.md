# Tutorial Hints (Contextual First-Use Hints) — Design

**Date:** 2026-05-24
**Status:** Approved (brainstorm), pending implementation plan
**Scope:** Phase 2 of the new-user experience effort. Covers **only** contextual
first-use hint bubbles. The app-wide polish pass and the docs/README rewrite are
separate, later cycles and are **out of scope** here.

**Branching:** This feature gates on `onboardingWelcomed`, a store introduced by
the Phase-1 onboarding work (PR #24, branch `feat/onboarding-setup-flow`, not yet
merged to `main`). It therefore branches off `feat/onboarding-setup-flow`
(stacked). After PR #24 merges to `main`, rebase `feat/tutorial-hints` onto
`main`.

## Problem

New users get no in-context explanation of non-obvious UI: the ⚡ background-agent
button, the host/recency filter pills, the terse session-action icons (↻ ✎ ♻ ×),
the terminal attachment. Today the only hints are 58 native `title=""` attributes
— invisible until hover, never shown proactively. We want **contextual,
dismissible "first-use" hints**: a small bubble that appears once, when a feature
first becomes relevant, with a "Got it" to dismiss it permanently.

## Decisions (from brainstorm)

- **Form:** contextual first-use hints (not a forced tour, not a hover-tooltip
  primitive replacement).
- **Pacing:** at most **one hint visible at a time**; a hint appears only when its
  feature becomes genuinely relevant (not a first-launch swarm).
- **Mechanism:** a **central layer + anchor action** (no floating-ui dependency —
  the codebase is deliberately dependency-light).

## Architecture

A data-driven hint system that mirrors the Phase-1 onboarding prefs pattern.
Target elements tag themselves with a Svelte action; a single layer renders the
one active bubble.

### `src/lib/hints.ts`

- `type HintId` — string union of the defined hint ids.
- `interface HintDef { id: HintId; text: string; placement: 'top' | 'bottom' | 'left' | 'right'; }`
- `const HINTS: HintDef[]` — the ordered hint list (priority = array order).
- **Persisted** via the existing `prefs.ts` helpers (`readPref`/`writePref`):
  - `hints-seen` → `string[]` of dismissed hint ids (stored as an array; used as a
    set).
  - `hints-enabled` → `boolean` master toggle, default `true`.
  - Both follow the auto-subscribe store pattern from `sessions.ts`
    (`showBgAgents`).
- **Registry:** a `writable<Map<HintId, HTMLElement>>` of anchors that are
  currently mounted **and** relevant. The `hintAnchor` action mutates it.
- **`pickActiveHint(order, registeredIds, seen, enabled, welcomed)`** — a **pure**
  function returning `HintId | null`: the first hint in `order` whose id is in
  `registeredIds`, not in `seen`, with `enabled === true` and `welcomed === true`.
  Returns `null` if hints are disabled, the welcome hasn't been dismissed, or none
  are eligible. Pure → unit-testable in isolation (like `deriveSteps`).
- `activeHintId` — a derived store combining `HINTS`, the registry,
  `hints-seen`, `hints-enabled`, and `onboardingWelcomed` through
  `pickActiveHint`.
- `markSeen(id: HintId)` — append to `hints-seen`.
- `resetHints()` — clear `hints-seen`.
- `hintsEnabled` store + setter (bound to the Settings toggle).

### `hintAnchor` Svelte action

`use:hintAnchor={{ id, when }}` where `when?: boolean` defaults to `true`.

- On mount (and when `when` is true): register `node` under `id` in the registry.
- On `when` flipping to `false`, or on unmount/`id` change: unregister.
- This is how **contextual relevance** is expressed — a hint is eligible only when
  its anchor is present *and* its condition holds. Conditions tied to DOM presence
  (a session row, the terminal header) need no `when`; conditions like "≥2 hosts"
  pass `when`.

### `src/lib/HintLayer.svelte`

Mounted once in `App.svelte`. Subscribes to `activeHintId`:

- When non-null, looks up the anchor element, reads `getBoundingClientRect()`, and
  positions a `position: fixed` bubble per the hint's `placement`, **clamped to the
  viewport** (reuse the clamp approach from the terminal context-menu,
  `TerminalView.svelte`). An arrow points at the anchor.
- Renders the bubble: hint `text`, a primary **"Got it"** button (→ `markSeen`),
  and a small **✕** (also `markSeen`).
- Recomputes position on `window` `resize` and `scroll` (capture). If the anchor's
  rect is zero-size or scrolled out of view, hide until it returns.
- Only ever one bubble (the active id is singular), so one-at-a-time is automatic.

### Settings (`SettingsDialog.svelte`)

Next to the existing "Replay setup guide" (added in Phase 1), add a **"Feature
hints"** row: a "Show feature hints" checkbox bound to `hintsEnabled`, and a
**"Reset hints"** button (→ `resetHints`, so dismissed hints can reappear).

## The hint set (initial)

Order = priority. Each is dismissed independently.

| id | Anchors to (component) | Relevant `when` | Text | placement |
|---|---|---|---|---|
| `host-filter` | host-filter pills (`Sidebar.svelte`) | ≥2 visible (non-hidden) hosts | "Filter sessions by machine — the dot shows reachability." | bottom |
| `bg-session` | ⚡ button (`Sidebar.svelte`, `[data-testid="new-bg-session-btn"]`) | ≥1 work session **and** 0 background sessions | "Launch a background agent: headless Claude that works without an attached terminal." | bottom |
| `session-actions` | the action-icon group of the **first** live session row (`Sidebar.svelte`) | a live session row exists | "Restart, rename, recreate, or kill a session from these icons." | bottom |
| `terminal-header` | terminal header (`TerminalView.svelte`, `[data-testid="terminal-header"]`) | a terminal is attached | "You're attached to this session's tmux. Right-click for copy / paste." | bottom |
| `recency-filter` | recency pills (`Sidebar.svelte`) | ≥1 session exists | "Narrow the list to recent activity." | bottom |

`session-actions` tags only the first session row's icon group (the action takes a
`when` of "this is the first row") so the bubble anchors to a single, stable
element rather than every row.

## Behavior & edge cases

- One bubble at a time. Dismiss ("Got it" or ✕) persists the id to `hints-seen`;
  the next eligible hint surfaces when its context next holds.
- Gated on `onboardingWelcomed` — brand-new users see the welcome/onboarding
  first; hints begin once they're actually using the app.
- Anchor unmounts or scrolls away while shown → the layer hides it; `activeHintId`
  recomputes (may surface the next eligible hint).
- `prefs.ts` already guards `typeof localStorage === 'undefined'`, so SSR/test
  environments degrade silently.
- The bubble uses CSS tokens (`--bg`, `--fg`, `--border`, `--accent`, `--fg-muted`)
  so dark/light theming is automatic.

## Testing

- **Vitest (unit):** `pickActiveHint` across cases — order priority; skips seen;
  returns `null` when disabled, when not yet welcomed, and when none registered;
  picks the first eligible. Persistence round-trip of `hints-seen` /
  `hints-enabled`; `markSeen` and `resetHints` behavior.
- **Not unit-tested:** the bubble's pixel positioning (jsdom has no layout) — it's
  covered by the manual pass below.
- **Manual:** with `onboardingWelcomed` set, trigger each hint's condition and
  confirm: bubble appears anchored correctly, one at a time; "Got it"/✕ dismiss and
  it doesn't return; "Reset hints" brings them back; "Show feature hints" off
  suppresses all; dark-mode styling is correct.

## Out of scope (future phases)

- App-wide visual polish pass.
- Docs / README rewrite.
- Replacing the 58 native `title=""` hover hints with a richer hover-tooltip
  primitive (a separate, non-tutorial concern).
- A guided sequential coachmark tour.
