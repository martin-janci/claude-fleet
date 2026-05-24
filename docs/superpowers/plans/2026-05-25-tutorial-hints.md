# Tutorial Hints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add contextual first-use hint bubbles — a small dismissible bubble appears once, when a UI feature first becomes relevant, with a "Got it" to dismiss it permanently.

**Architecture:** A data-driven, central-layer design. A `hints.ts` module holds the ordered hint list, two persisted flags (`hints-seen`, `hints-enabled`), a registry of currently-relevant anchors (populated by a `use:hintAnchor` Svelte action), and a **pure** `pickActiveHint()` that selects the single active hint. One `HintLayer` mounted in `App.svelte` positions the active bubble (pure `computeBubblePosition()`), reusing the terminal context-menu's viewport-clamp idea — no floating-ui dependency.

**Tech Stack:** Svelte 5 runes + TypeScript, Svelte actions, Vitest.

**Spec:** `docs/specs/2026-05-24-tutorial-hints-design.md`

**Branch:** `feat/tutorial-hints` (already created, stacked on `feat/onboarding-setup-flow`; the spec is committed there). Rebase onto `main` after PR #24 merges.

---

## Conventions for this plan

- Repo root: `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3`. All commands run from there.
- Tests: `npx vitest run <path>`. Type-check: `npx svelte-check --tsconfig ./tsconfig.json`.
- The codebase has **no existing `use:` actions** — this introduces the first one (standard Svelte 5; an action is `(node: HTMLElement, params) => { update?, destroy? }`).
- The persisted-store pattern to copy is in `src/lib/sessions.ts` (`showBgAgents`): `writable(readPref(key, fallback, isBool))` + a module-level `.subscribe(v => writePref(key, v))`. Prefs helpers are in `src/lib/prefs.ts`.
- `onboardingWelcomed` is exported from `src/lib/onboarding.ts` (present on this branch).

---

## File structure

**Create:**
- `src/lib/hints.ts` — types, `HINTS`, persisted stores, anchor registry, `hintAnchor` action, pure `pickActiveHint`, derived `activeHintId`, `markSeen`/`resetHints`.
- `src/lib/hints.test.ts` — Vitest for `pickActiveHint` + persistence helpers.
- `src/lib/hintPosition.ts` — pure `computeBubblePosition()`.
- `src/lib/hintPosition.test.ts` — Vitest for positioning math.
- `src/lib/HintLayer.svelte` — renders the active bubble.

**Modify:**
- `src/App.svelte` — mount `<HintLayer />` once.
- `src/lib/Sidebar.svelte` — tag 4 anchors with `use:hintAnchor`.
- `src/lib/TerminalView.svelte` — tag the terminal header anchor.
- `src/lib/SettingsDialog.svelte` — "Feature hints" toggle + "Reset hints".

---

## Task 1: hints.ts — registry, pure selection, persistence, action

**Files:**
- Create: `src/lib/hints.ts`
- Test: `src/lib/hints.test.ts`

- [ ] **Step 1: Create `src/lib/hints.ts`**

```ts
import { writable, derived, get, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { onboardingWelcomed } from './onboarding';

export type HintId =
  | 'host-filter'
  | 'bg-session'
  | 'session-actions'
  | 'terminal-header'
  | 'recency-filter';

export type Placement = 'top' | 'bottom' | 'left' | 'right';

export interface HintDef {
  id: HintId;
  text: string;
  placement: Placement;
}

/** Ordered by priority — earlier hints win when several are eligible at once. */
export const HINTS: HintDef[] = [
  {
    id: 'host-filter',
    text: 'Filter sessions by machine — the dot shows reachability.',
    placement: 'bottom',
  },
  {
    id: 'bg-session',
    text: 'Launch a background agent: headless Claude that works without an attached terminal.',
    placement: 'top',
  },
  {
    id: 'session-actions',
    text: 'Restart, rename, recreate, or kill a session from these icons.',
    placement: 'bottom',
  },
  {
    id: 'terminal-header',
    text: "You're attached to this session's tmux. Right-click for copy / paste.",
    placement: 'bottom',
  },
  {
    id: 'recency-filter',
    text: 'Narrow the list to recent activity.',
    placement: 'bottom',
  },
];

// ---- Persisted state --------------------------------------------------------

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
const isStringArray = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === 'string');

/** Ids the user has dismissed (persisted). */
export const seenHints = writable<string[]>(readPref('hints-seen', [], isStringArray));
seenHints.subscribe((v) => writePref('hints-seen', v));

/** Master on/off toggle for all feature hints (persisted, default on). */
export const hintsEnabled = writable<boolean>(readPref('hints-enabled', true, isBool));
hintsEnabled.subscribe((v) => writePref('hints-enabled', v));

export function markSeen(id: HintId): void {
  seenHints.update((s) => (s.includes(id) ? s : [...s, id]));
}

export function resetHints(): void {
  seenHints.set([]);
}

// ---- Anchor registry --------------------------------------------------------

// Set of hint ids whose anchor is currently mounted AND relevant — drives
// reactivity. The actual elements live in a plain Map for lookup by HintLayer.
const registeredIds = writable<Set<HintId>>(new Set());
const anchorEls = new Map<HintId, HTMLElement>();

function register(id: HintId, node: HTMLElement): void {
  anchorEls.set(id, node);
  registeredIds.update((s) => {
    if (s.has(id)) return s;
    const n = new Set(s);
    n.add(id);
    return n;
  });
}

function unregister(id: HintId, node: HTMLElement): void {
  // Only clear if THIS node is the current holder (multiple rows may share an id,
  // e.g. session-actions — last mount wins, and only it unregisters).
  if (anchorEls.get(id) !== node) return;
  anchorEls.delete(id);
  registeredIds.update((s) => {
    const n = new Set(s);
    n.delete(id);
    return n;
  });
}

/** Look up the live anchor element for a hint (used by HintLayer). */
export function anchorEl(id: HintId): HTMLElement | undefined {
  return anchorEls.get(id);
}

export interface HintAnchorParams {
  id: HintId;
  /** When false, the feature isn't relevant yet — anchor is not registered. */
  when?: boolean;
}

/** Svelte action: tag an element as a hint anchor. `use:hintAnchor={{ id, when }}`. */
export function hintAnchor(node: HTMLElement, params: HintAnchorParams) {
  let { id, when = true } = params;
  if (when) register(id, node);
  return {
    update(next: HintAnchorParams) {
      const nextWhen = next.when ?? true;
      if (next.id === id && nextWhen === when) return;
      // Drop the old registration, apply the new one.
      unregister(id, node);
      id = next.id;
      when = nextWhen;
      if (when) register(id, node);
    },
    destroy() {
      unregister(id, node);
    },
  };
}

// ---- Active-hint selection --------------------------------------------------

/**
 * Pure selection: the first hint in `order` that is registered, unseen, with
 * hints enabled and the welcome already dismissed. `null` otherwise.
 */
export function pickActiveHint(
  order: readonly HintId[],
  registered: ReadonlySet<HintId>,
  seen: readonly string[],
  enabled: boolean,
  welcomed: boolean,
): HintId | null {
  if (!enabled || !welcomed) return null;
  for (const id of order) {
    if (registered.has(id) && !seen.includes(id)) return id;
  }
  return null;
}

const order = HINTS.map((h) => h.id);

export const activeHintId: Readable<HintId | null> = derived(
  [registeredIds, seenHints, hintsEnabled, onboardingWelcomed],
  ([reg, seen, enabled, welcomed]) =>
    pickActiveHint(order, reg, seen, enabled, welcomed),
);

/** Look up a hint definition by id. */
export function hintDef(id: HintId): HintDef | undefined {
  return HINTS.find((h) => h.id === id);
}

// re-export for consumers that need a one-shot read
export { get };
```

- [ ] **Step 2: Create `src/lib/hints.test.ts`**

```ts
import { describe, it, expect } from 'vitest';
import { pickActiveHint, type HintId } from './hints';

const ORDER: HintId[] = [
  'host-filter',
  'bg-session',
  'session-actions',
  'terminal-header',
  'recency-filter',
];

describe('pickActiveHint', () => {
  it('returns null when hints are disabled', () => {
    expect(pickActiveHint(ORDER, new Set(['host-filter']), [], false, true)).toBeNull();
  });

  it('returns null before the welcome is dismissed', () => {
    expect(pickActiveHint(ORDER, new Set(['host-filter']), [], true, false)).toBeNull();
  });

  it('returns null when nothing is registered', () => {
    expect(pickActiveHint(ORDER, new Set(), [], true, true)).toBeNull();
  });

  it('picks the first registered, unseen hint in priority order', () => {
    const reg = new Set<HintId>(['recency-filter', 'bg-session']);
    expect(pickActiveHint(ORDER, reg, [], true, true)).toBe('bg-session');
  });

  it('skips seen hints', () => {
    const reg = new Set<HintId>(['bg-session', 'session-actions']);
    expect(pickActiveHint(ORDER, reg, ['bg-session'], true, true)).toBe('session-actions');
  });

  it('returns null when all registered hints are seen', () => {
    const reg = new Set<HintId>(['host-filter']);
    expect(pickActiveHint(ORDER, reg, ['host-filter'], true, true)).toBeNull();
  });
});
```

- [ ] **Step 3: Run tests**

Run: `npx vitest run src/lib/hints.test.ts`
Expected: 6 tests PASS.

- [ ] **Step 4: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors in `hints.ts` / `hints.test.ts`. VERIFY the `onboardingWelcomed` import path resolves (it's exported from `src/lib/onboarding.ts`); if the export name differs, fix the import and report.

- [ ] **Step 5: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add src/lib/hints.ts src/lib/hints.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "feat(hints): registry, anchor action, and pure active-hint selection"
```

---

## Task 2: hintPosition.ts — pure bubble positioning

**Files:**
- Create: `src/lib/hintPosition.ts`
- Test: `src/lib/hintPosition.test.ts`

- [ ] **Step 1: Create `src/lib/hintPosition.ts`**

```ts
import type { Placement } from './hints';

export interface Rect {
  top: number;
  left: number;
  width: number;
  height: number;
}

export interface Pos {
  top: number;
  left: number;
}

const GAP = 8; // px between anchor and bubble
const MARGIN = 6; // px min distance from viewport edge

/**
 * Position a bubble of size `bubbleW`×`bubbleH` relative to `anchor` for the
 * given `placement`, clamped to stay within the `vw`×`vh` viewport. Pure — all
 * inputs are plain numbers so it is unit-testable without a DOM.
 */
export function computeBubblePosition(
  anchor: Rect,
  placement: Placement,
  bubbleW: number,
  bubbleH: number,
  vw: number,
  vh: number,
): Pos {
  let top: number;
  let left: number;

  switch (placement) {
    case 'top':
      top = anchor.top - bubbleH - GAP;
      left = anchor.left + anchor.width / 2 - bubbleW / 2;
      break;
    case 'left':
      top = anchor.top + anchor.height / 2 - bubbleH / 2;
      left = anchor.left - bubbleW - GAP;
      break;
    case 'right':
      top = anchor.top + anchor.height / 2 - bubbleH / 2;
      left = anchor.left + anchor.width + GAP;
      break;
    case 'bottom':
    default:
      top = anchor.top + anchor.height + GAP;
      left = anchor.left + anchor.width / 2 - bubbleW / 2;
      break;
  }

  // Clamp to viewport.
  left = Math.max(MARGIN, Math.min(left, vw - bubbleW - MARGIN));
  top = Math.max(MARGIN, Math.min(top, vh - bubbleH - MARGIN));
  return { top, left };
}
```

- [ ] **Step 2: Create `src/lib/hintPosition.test.ts`**

```ts
import { describe, it, expect } from 'vitest';
import { computeBubblePosition, type Rect } from './hintPosition';

const anchor: Rect = { top: 100, left: 100, width: 40, height: 20 };

describe('computeBubblePosition', () => {
  it('places a bottom bubble below and horizontally centered on the anchor', () => {
    const p = computeBubblePosition(anchor, 'bottom', 200, 80, 1000, 1000);
    expect(p.top).toBe(100 + 20 + 8); // below with gap
    expect(p.left).toBe(100 + 20 - 100); // center: anchorCenterX - bubbleW/2 = 120 - 100
  });

  it('places a top bubble above the anchor', () => {
    const p = computeBubblePosition(anchor, 'top', 200, 80, 1000, 1000);
    expect(p.top).toBe(100 - 80 - 8);
  });

  it('clamps a bubble that would overflow the right edge', () => {
    const right: Rect = { top: 100, left: 980, width: 40, height: 20 };
    const p = computeBubblePosition(right, 'bottom', 200, 80, 1000, 1000);
    expect(p.left).toBe(1000 - 200 - 6); // vw - bubbleW - MARGIN
  });

  it('clamps a bubble that would overflow the top edge', () => {
    const top: Rect = { top: 2, left: 100, width: 40, height: 20 };
    const p = computeBubblePosition(top, 'top', 200, 80, 1000, 1000);
    expect(p.top).toBe(6); // MARGIN
  });
});
```

- [ ] **Step 3: Run tests**

Run: `npx vitest run src/lib/hintPosition.test.ts`
Expected: 4 tests PASS.

- [ ] **Step 4: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add src/lib/hintPosition.ts src/lib/hintPosition.test.ts
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "feat(hints): pure viewport-clamped bubble positioning"
```

---

## Task 3: HintLayer.svelte — render the active bubble

**Files:**
- Create: `src/lib/HintLayer.svelte`

- [ ] **Step 1: Create `src/lib/HintLayer.svelte`**

```svelte
<script lang="ts">
  import { activeHintId, anchorEl, hintDef, markSeen } from './hints';
  import { computeBubblePosition, type Pos } from './hintPosition';

  // Measured bubble size (read after render); start with sane defaults.
  let bubbleEl = $state<HTMLDivElement | null>(null);
  let pos = $state<Pos | null>(null);

  const def = $derived($activeHintId ? hintDef($activeHintId) : undefined);

  function reposition() {
    const id = $activeHintId;
    if (!id || !def) {
      pos = null;
      return;
    }
    const el = anchorEl(id);
    if (!el) {
      pos = null;
      return;
    }
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) {
      pos = null; // anchor not laid out / hidden
      return;
    }
    const bw = bubbleEl?.offsetWidth ?? 208;
    const bh = bubbleEl?.offsetHeight ?? 90;
    pos = computeBubblePosition(
      { top: r.top, left: r.left, width: r.width, height: r.height },
      def.placement,
      bw,
      bh,
      window.innerWidth,
      window.innerHeight,
    );
  }

  // Recompute when the active hint changes, and on resize/scroll while shown.
  $effect(() => {
    // touch reactive deps so this re-runs when they change
    void $activeHintId;
    void def;
    reposition();
    if (!$activeHintId) return;
    const handler = () => reposition();
    window.addEventListener('resize', handler);
    window.addEventListener('scroll', handler, true); // capture: catch inner scrollers
    return () => {
      window.removeEventListener('resize', handler);
      window.removeEventListener('scroll', handler, true);
    };
  });

  function dismiss() {
    if ($activeHintId) markSeen($activeHintId);
  }
</script>

{#if $activeHintId && def && pos}
  <div
    class="hint"
    bind:this={bubbleEl}
    style="top:{pos.top}px; left:{pos.left}px;"
    role="status"
    data-testid="hint-bubble"
    data-hint-id={$activeHintId}
  >
    <div class="htext">{def.text}</div>
    <div class="hactions">
      <button class="gotit" onclick={dismiss}>Got it</button>
      <button class="x" onclick={dismiss} aria-label="Dismiss hint">✕</button>
    </div>
  </div>
{/if}

<style>
  .hint {
    position: fixed;
    z-index: 30;
    width: 208px;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 11px;
    box-shadow: 0 6px 22px rgba(0, 0, 0, 0.16);
    font-size: 0.75rem;
    line-height: 1.45;
  }
  .htext {
    color: var(--fg);
    margin-bottom: 8px;
  }
  .hactions {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .gotit {
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 0.75rem;
    cursor: pointer;
  }
  .x {
    background: none;
    border: none;
    color: var(--fg-muted, #777);
    font-size: 0.75rem;
    cursor: pointer;
  }
</style>
```

- [ ] **Step 2: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors for `HintLayer.svelte`. (An a11y warning on the bubble is acceptable — but it uses `role="status"` and real `<button>`s, so it should be clean. The bubble's pixel positioning is not unit-tested; jsdom has no layout.)

- [ ] **Step 3: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add src/lib/HintLayer.svelte
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "feat(hints): HintLayer renders the active bubble"
```

---

## Task 4: Wire HintLayer + tag anchors

**Files:**
- Modify: `src/App.svelte`, `src/lib/Sidebar.svelte`, `src/lib/TerminalView.svelte`

- [ ] **Step 1: Mount `HintLayer` in `App.svelte`**

Add to the imports near the other lib imports (e.g. after the `WelcomeDialog` import):
```ts
  import HintLayer from './lib/HintLayer.svelte';
```
In the template, add `<HintLayer />` near where `<WelcomeDialog>` is rendered (top level, sibling to the panes — it is `position: fixed`, so placement in markup doesn't matter):
```svelte
<HintLayer />
```

- [ ] **Step 2: Tag the Sidebar anchors**

In `src/lib/Sidebar.svelte`, add the import near the other store imports:
```ts
  import { hintAnchor } from './hints';
```

Then add `use:hintAnchor` to four elements (read each region first to confirm the lines haven't shifted):

(a) The host-filter `<nav class="hosts" ...>` (~line 702) — relevant when 2+ visible hosts:
```svelte
    <nav
      class="hosts"
      aria-label="host filter"
      use:hintAnchor={{ id: 'host-filter', when: $hosts.filter((h) => !h.hidden).length >= 2 }}
    >
```

(b) The recency `<nav class="recency" ...>` (~line 729) — relevant when any session exists:
```svelte
    <nav
      class="recency"
      aria-label="recency filter"
      use:hintAnchor={{ id: 'recency-filter', when: $sessions.length > 0 }}
    >
```

(c) The ⚡ background-session button (~line 852, `data-testid="new-bg-session-btn"`) — relevant when there is ≥1 work session and no background session:
```svelte
      <button
        class="icon-btn"
        title="Launch a supervised Claude background session"
        onclick={() => (showBgModal = true)}
        data-testid="new-bg-session-btn"
        use:hintAnchor={{
          id: 'bg-session',
          when:
            $sessions.some((s) => s.kind !== 'bg') && !$sessions.some((s) => s.kind === 'bg'),
        }}
      >⚡</button>
```

(d) The session-row actions `<div class="row-actions">` (~line 635, inside the `sessionRow` snippet) — relevant whenever a row renders (no `when`; multiple rows share the id, last-mount wins via the registry):
```svelte
          <div class="row-actions" use:hintAnchor={{ id: 'session-actions' }}>
```

VERIFY `$sessions` is already imported/available in Sidebar.svelte (it is — the file imports the `sessions` store). If `$hosts` / `$sessions` are referenced via different local names, match them.

- [ ] **Step 3: Tag the terminal header anchor**

In `src/lib/TerminalView.svelte`, add the import (near the top of the `<script>`):
```ts
  import { hintAnchor } from './hints';
```
Add the action to the header div (~line 849, `data-testid="terminal-header"`) — relevant whenever the terminal is shown (no `when`):
```svelte
    <div class="header" data-testid="terminal-header" use:hintAnchor={{ id: 'terminal-header' }}>
```

- [ ] **Step 4: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors. Resolve any local-name mismatches surfaced here.

- [ ] **Step 5: Run the full frontend suite**

Run: `npx vitest run`
Expected: all green. (Mounting `HintLayer` is harmless in `App.test.ts`: `onboardingWelcomed` defaults false in tests → `activeHintId` is null → no bubble renders. Tagging anchors in `Sidebar.svelte` just registers ids → harmless. If any test newly fails, report it — do NOT mask a real failure.)

- [ ] **Step 6: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add src/App.svelte src/lib/Sidebar.svelte src/lib/TerminalView.svelte
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "feat(hints): mount HintLayer and tag UI anchors"
```

---

## Task 5: Settings — Feature hints toggle + reset

**Files:**
- Modify: `src/lib/SettingsDialog.svelte`

- [ ] **Step 1: Add the control**

In `src/lib/SettingsDialog.svelte`, add to the `<script>` imports (next to the existing `onboardingDismissed`/`onboardingWelcomed` import):
```ts
  import { hintsEnabled, resetHints } from './hints';
```

There is an existing onboarding section (`<section class="block" data-testid="onboarding-section">` ~line 221) that uses `.hook-section` / `.hook-btn` classes. Add a "Feature hints" control inside (or just after) that section, reusing those classes. Read the section first, then add:
```svelte
      <div class="hook-section">
        <label class="hint-toggle">
          <input type="checkbox" bind:checked={$hintsEnabled} />
          Show feature hints
        </label>
        <button class="hook-btn" onclick={resetHints} data-testid="reset-hints">
          Reset hints
        </button>
      </div>
```
If the file has an existing label/checkbox style, reuse it instead of `.hint-toggle`; otherwise add a minimal rule:
```css
  .hint-toggle { display: flex; align-items: center; gap: 6px; font-size: 0.85rem; }
```
Report which classes you reused.

- [ ] **Step 2: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors. (`bind:checked={$hintsEnabled}` binds the store directly — valid in Svelte 5.)

- [ ] **Step 3: Run the suite**

Run: `npx vitest run`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add src/lib/SettingsDialog.svelte
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "feat(hints): Settings toggle to show/reset feature hints"
```

---

## Task 6: Full verification

- [ ] **Step 1: Frontend suite + type-check**

```bash
npx vitest run
npx svelte-check --tsconfig ./tsconfig.json
```
Expected: all tests pass (incl. `hints.test.ts` 6, `hintPosition.test.ts` 4); 0 svelte-check errors (pre-existing a11y warnings on other dialogs are fine — compare to the branch point if unsure).

- [ ] **Step 2: Manual walkthrough (if a dev build runs)**

`pnpm tauri dev`. With `onboarding-welcomed` already set (dismiss the welcome, or toggle it in localStorage):
- Add a 2nd host → the `host-filter` bubble appears under the host pills; "Got it" dismisses it and it doesn't return.
- With a session but no bg agent → the `bg-session` bubble points at ⚡.
- A session row shows → the `session-actions` bubble points at the row icons.
- Open a session → the `terminal-header` bubble appears.
- Confirm only one bubble shows at a time; Settings → "Show feature hints" off hides them; "Reset hints" brings dismissed ones back; dark-mode styling looks right.

- [ ] **Step 3: Final commit (if any cleanup)**

```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add -A
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "chore(hints): verification cleanup" || echo "nothing to commit"
```

---

## Self-review notes (addressed)

- **Spec coverage:** `hints.ts` (registry, persisted `hints-seen`/`hints-enabled`, `pickActiveHint`, `activeHintId`, `markSeen`/`resetHints`, `hintAnchor` action) — Task 1; `computeBubblePosition` — Task 2; `HintLayer` bubble with "Got it"/✕, resize/scroll reposition, anchor-gone hiding — Task 3; the 5 hints + their `when` relevance + `onboardingWelcomed` gate (in `pickActiveHint`/`activeHintId`) — Tasks 1 & 4; Settings toggle + reset — Task 5; tests — Tasks 1, 2, 6.
- **Type consistency:** `HintId`, `Placement`, `HintDef`, `pickActiveHint(order, registered, seen, enabled, welcomed)`, `computeBubblePosition(anchor, placement, bw, bh, vw, vh)`, `anchorEl`, `hintAnchor({id, when})` are used identically across `hints.ts`, `hintPosition.ts`, `HintLayer.svelte`, and the test files.
- **Placeholder scan:** none.
- **Verification points flagged inline (not placeholders):** the `onboardingWelcomed` export name (Task 1 Step 4) and the `$hosts`/`$sessions` local names in Sidebar (Task 4 Step 2) — both checked against existing code by the implementer.
- **Out of scope (per spec):** hover-`title` replacement, guided tour, polish pass, docs/README.
```
