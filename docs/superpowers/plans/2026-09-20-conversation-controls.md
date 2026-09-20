# Conversation control system Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the conversation surfaces one control vocabulary, one sticky bar, and a composer shell that can later hold attachments.

**Architecture:** A geometry + colour token layer in `src/app.css`, mirrored by a TS table that a contrast test asserts against; four global control primitives in a new `src/lib/controls.css`; then the three conversation surfaces and the shared controls converted onto them. No framework, no utility classes — the app's token layer already exists and simply stops at colour.

**Tech Stack:** Svelte 5 runes, TypeScript, Vitest + `@testing-library/svelte`, plain CSS custom properties.

**Spec:** `docs/superpowers/specs/2026-09-20-conversation-controls-and-attachments-design.md` (stages 0–4)

**Companion plan:** `docs/superpowers/plans/2026-09-20-conversation-attachments.md` (stages 5–6). That plan depends on Task 9 here — the composer shell is the container attachments live in. Both land in one PR.

## Global Constraints

- **Every existing `data-testid` must survive**, on an element with the same role and behaviour: `conv-header`, `conv-find-button`, `conv-find-input`, `conv-find-count`, `conv-find-prev`, `conv-find-next`, `conv-find-close`, `conv-turns-button`, `conv-turn-index`, `conv-turn-index-item`, `conv-composer-send`, `conv-chip`, `conv-chip-enter`, `conv-model`, `conv-status`, `conv-ctx`. Changing an assertion means the behaviour changed — justify it in the commit, do not absorb it.
- **Run the full suite, never a filtered one.** `npx vitest run` and `npx svelte-check --tsconfig ./tsconfig.json`. (`pnpm test` / `pnpm check` fail here — the binaries are not on PATH.)
- **Run `pnpm install --frozen-lockfile` before the first test run.** Stale `node_modules` produce `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"`, which is a dependency gap, not a code error.
- **Never run a dev build of the desktop app on this machine.** The singleton guard SIGTERMs the installed app and, with the real `HOME`, migrates the production `state.db` irreversibly. Verification is Vitest only.
- **Contrast floors:** text ≥ 4.5:1, non-text ≥ 3:1, measured against `--bg-pane` (the surface these controls sit on).
- **Opacity on a disabled control never goes below `0.55`.**
- **Nothing but `:focus-visible` gets a 2px ring, in any colour, anywhere.**
- **`--radius-pill` is reserved** for `aria-pressed` toggles and the context meter.
- Commit per task, Conventional Commits, no attribution trailers.

---

### Task 1: The context meter's healthy colour becomes a token

`attention.ts` returns a hardcoded `#50c86e` for the healthy meter. On `--bg-pane` `#fafafa` that is 2.047:1 — below 4.5:1 for the 9.5px `.ctx-pct` text and below 3:1 for the bar. Dark mode is 8.47:1, which is why it survived.

**Files:**
- Modify: `src/app.css` (all four `:root` blocks)
- Modify: `src/lib/attention.ts:90-97`
- Test: `src/lib/attention.test.ts:112`

**Interfaces:**
- Consumes: nothing.
- Produces: CSS variable `--usage-ok`; `contextColor('ok')` now returns `'var(--usage-ok)'`.

- [ ] **Step 1: Change the assertion to the value we want**

In `src/lib/attention.test.ts`, replace line 112:

```ts
    expect(contextColor('ok')).toBe('var(--usage-ok)');
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/lib/attention.test.ts`
Expected: FAIL — `expected '#50c86e' to be 'var(--usage-ok)'`

- [ ] **Step 3: Add the token to all four `:root` blocks**

`src/app.css` has four blocks that duplicate the palette: `:root`, the `@media (prefers-color-scheme: dark) { :root }` block, `:root[data-theme='light']` and `:root[data-theme='dark']`. Add to the two light blocks, next to `--usage-warn`:

```css
  /* Healthy context. 4.91:1 vs --bg-pane #fafafa (5.13:1 vs --bg #fff). */
  --usage-ok: #2e7d32;
```

Add to the two dark blocks:

```css
  /* vs --bg-pane #161616: 9.37:1. */
  --usage-ok: #5dd17a;
```

- [ ] **Step 4: Return the token**

`src/lib/attention.ts`, in `contextColor`:

```ts
    case 'ok':
      return 'var(--usage-ok)';
```

Update the doc comment above the function: it currently says warn/crit use theme tokens, implying `ok` does not. Replace that clause with "every level uses the theme tokens shared with the usage bars".

- [ ] **Step 5: Run the full suite**

Run: `npx vitest run`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/app.css src/lib/attention.ts src/lib/attention.test.ts
git commit -m "fix(a11y): the healthy context meter uses a token, not a 2:1 hex"
```

---

### Task 2: A contrast harness, so the next hardcoded hex cannot ship

Task 1 fixed one instance. This fixes the class: the token values live in a TS table that `app.css` mirrors, and a test computes WCAG relative luminance for every documented pair.

**Files:**
- Create: `src/lib/tokens.ts`
- Create: `src/lib/tokens.test.ts`

**Interfaces:**
- Consumes: the `--usage-ok` values from Task 1.
- Produces: `relativeLuminance(hex: string): number`, `contrastRatio(a: string, b: string): number`, `THEME: Record<'light' | 'dark', Record<string, string>>`, `CONTRAST_PAIRS: ContrastPair[]` where `ContrastPair = { fg: string; bg: string; min: number; note: string }`. Task 4 extends `THEME` and `CONTRAST_PAIRS`; nothing else imports this module at runtime.

- [ ] **Step 1: Write the failing test**

Create `src/lib/tokens.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { relativeLuminance, contrastRatio, THEME, CONTRAST_PAIRS } from './tokens';

describe('contrast maths', () => {
  it('matches known WCAG values', () => {
    expect(relativeLuminance('#ffffff')).toBeCloseTo(1, 5);
    expect(relativeLuminance('#000000')).toBeCloseTo(0, 5);
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 2);
    // The bug Task 1 fixed, kept as a regression witness.
    expect(contrastRatio('#50c86e', '#fafafa')).toBeCloseTo(2.05, 2);
  });
});

describe('every documented token pair clears its floor', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const pair of CONTRAST_PAIRS) {
      it(`${mode}: ${pair.fg} on ${pair.bg} >= ${pair.min}:1 (${pair.note})`, () => {
        const fg = THEME[mode][pair.fg];
        const bg = THEME[mode][pair.bg];
        expect(fg, `${pair.fg} missing from THEME.${mode}`).toBeTruthy();
        expect(bg, `${pair.bg} missing from THEME.${mode}`).toBeTruthy();
        expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(pair.min);
      });
    }
  }
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `npx vitest run src/lib/tokens.test.ts`
Expected: FAIL — `Failed to resolve import "./tokens"`

- [ ] **Step 3: Write the module**

Create `src/lib/tokens.ts`:

```ts
/**
 * The theme palette as data, mirroring the four `:root` blocks in app.css,
 * so the contrast floors those blocks claim in comments are actually
 * asserted. Nothing imports this at runtime — it exists to be tested.
 *
 * When you add or change a token in app.css, change it here too and add the
 * pair it has to clear to CONTRAST_PAIRS.
 */

export interface ContrastPair {
  /** Key in THEME for the foreground / border colour. */
  fg: string;
  /** Key in THEME for the surface it sits on. */
  bg: string;
  /** WCAG floor: 4.5 for text, 3 for non-text. */
  min: number;
  note: string;
}

export const THEME: Record<'light' | 'dark', Record<string, string>> = {
  light: {
    bg: '#ffffff',
    'bg-pane': '#fafafa',
    fg: '#1a1a1a',
    'fg-muted': '#6b6b6b',
    border: '#e5e5e5',
    accent: '#2563eb',
    'usage-ok': '#2e7d32',
    'usage-warn': '#b45309',
    'usage-crit': '#c62828',
  },
  dark: {
    bg: '#0f0f0f',
    'bg-pane': '#161616',
    fg: '#ededed',
    'fg-muted': '#999999',
    border: '#262626',
    accent: '#60a5fa',
    'usage-ok': '#5dd17a',
    'usage-warn': '#d29b4a',
    'usage-crit': '#ef5350',
  },
};

export const CONTRAST_PAIRS: ContrastPair[] = [
  { fg: 'fg', bg: 'bg-pane', min: 4.5, note: 'body text' },
  { fg: 'fg-muted', bg: 'bg-pane', min: 4.5, note: 'muted text' },
  { fg: 'usage-ok', bg: 'bg-pane', min: 4.5, note: 'healthy context meter' },
  { fg: 'usage-warn', bg: 'bg-pane', min: 4.5, note: 'warn text and bars' },
  { fg: 'usage-crit', bg: 'bg-pane', min: 4.5, note: 'crit text and bars' },
  { fg: 'accent', bg: 'bg', min: 3, note: 'focus ring' },
];

function channel(v: number): number {
  const c = v / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

export function relativeLuminance(hex: string): number {
  const h = hex.replace('#', '');
  if (h.length !== 6) throw new Error(`expected #rrggbb, got ${hex}`);
  const r = channel(parseInt(h.slice(0, 2), 16));
  const g = channel(parseInt(h.slice(2, 4), 16));
  const b = channel(parseInt(h.slice(4, 6), 16));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `npx vitest run src/lib/tokens.test.ts`
Expected: PASS — 1 maths test plus 12 pair tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/tokens.ts src/lib/tokens.test.ts
git commit -m "test(theme): assert the contrast floors app.css claims in comments"
```

---

### Task 3: `--mono` and a class for `conv-retry`

Two independent one-liners that need no new behaviour. `var(--mono)` is read at 16 sites and defined nowhere, so every one of them silently falls through to its fallback; a further 59 declarations hardcode the stack independently. `conv-retry` has no class and no global `button` rule exists, so it renders as the only native macOS push button in the panel — on the error path, ignoring dark mode.

**Files:**
- Modify: `src/app.css` (`:root` only — a font stack is not per-theme)
- Modify: `src/lib/ConversationPanel.svelte:1052`, and its `<style>` block

**Interfaces:**
- Consumes: nothing.
- Produces: CSS variable `--mono`.

- [ ] **Step 1: Define the token**

In `src/app.css`, in the first `:root` block only:

```css
  /* Read at 16 sites that have been falling through to their fallback
     since the token was designed and never shipped. 59 more hardcode the
     stack; they are not this task's business. */
  --mono: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
```

- [ ] **Step 2: Give the Retry button a class**

`src/lib/ConversationPanel.svelte`, the error row:

```svelte
            <button type="button" class="retry-btn" data-testid="conv-retry" onclick={() => void load()}>Retry</button>
```

And in the `<style>` block, next to `.error-row`:

```css
  .retry-btn {
    padding: 0.1rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    font-size: 0.74rem;
    cursor: pointer;
  }
  .retry-btn:hover { border-color: var(--accent); }
```

(Task 5 replaces this rule with `.btn .btn--quiet.is-bounded`. It is written out here so the error path is not left native for the intervening tasks.)

- [ ] **Step 3: Run the full suite**

Run: `npx vitest run`
Expected: PASS, unchanged count — `conv-retry` is asserted by testid, not by class.

- [ ] **Step 4: Commit**

```bash
git add src/app.css src/lib/ConversationPanel.svelte
git commit -m "fix(ui): define --mono, and stop Retry rendering as a native button"
```

---

### Task 4: A dropped file must not navigate the webview

In a WKWebView an unhandled drop sends the window to `file://…`. There is no router and no recovery: the app state is gone. This is a live bug before any attachment work exists, and the attachment plan depends on it.

**Files:**
- Modify: `src/App.svelte` (the `onMount(() => {…})` at line 264)
- Test: `src/App.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: a window-level `dragover`/`drop` swallow. The attachment plan's shell handler calls `stopPropagation()` so this listener does not undo a real drop.

- [ ] **Step 1: Write the failing test**

Append to `src/App.test.ts`:

```ts
  it('swallows a drop outside a drop target so the webview cannot navigate', async () => {
    render(App);
    await tick();
    const ev = new Event('drop', { bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);

    const over = new Event('dragover', { bubbles: true, cancelable: true });
    window.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
  });
```

Add `tick` to the existing `svelte` import in that file if it is not already there.

- [ ] **Step 2: Run it to verify it fails**

Run: `npx vitest run src/App.test.ts`
Expected: FAIL — `expected false to be true`

- [ ] **Step 3: Install the guard**

In `src/App.svelte`, inside the synchronous `onMount(() => {…})` (it has a bare void body — the listeners it registers are torn down by the paired `onDestroy` a few lines below, not by a returned cleanup), add:

```ts
    // A drop that reaches the window navigates a WKWebView to file://… and
    // takes the whole app state with it: no router, no recovery. Drop
    // targets call stopPropagation(), so this only ever sees strays.
    const swallowDrag = (e: DragEvent) => e.preventDefault();
    window.addEventListener('dragover', swallowDrag);
    window.addEventListener('drop', swallowDrag);
```

and in that paired `onDestroy`, alongside the other teardowns already there:

```ts
      window.removeEventListener('dragover', swallowDrag);
      window.removeEventListener('drop', swallowDrag);
```

- [ ] **Step 4: Run the full suite**

Run: `npx vitest run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/App.svelte src/App.test.ts
git commit -m "fix(ui): a stray file drop no longer navigates the webview away"
```

---

### Task 5: The token layer and `controls.css`

Purely additive: no component changes appearance in this task. It adds the geometry tokens, the control colour tokens, and the four primitives.

**Files:**
- Modify: `src/app.css` (geometry on the first `:root`; colours in all four blocks)
- Create: `src/lib/controls.css`
- Modify: `src/main.ts`
- Modify: `src/lib/tokens.ts`, `src/lib/tokens.test.ts` (extend the table and pairs)

**Interfaces:**
- Consumes: `THEME` / `CONTRAST_PAIRS` from Task 2.
- Produces: global classes `.btn`, `.btn--primary`, `.btn--quiet`, `.is-bounded`, `.btn--toggle`, `.btn--icon`, `.btn--warn`, `.btn--crit`, `.btn-group`, `.tag`, `.tag--mono`. Tasks 6–14 consume these by name.

- [ ] **Step 1: Extend the token table with the new pairs (failing test first)**

In `src/lib/tokens.ts`, add to `THEME.light`:

```ts
    'control-bg': '#ffffff',
    'control-bg-hover': '#f0f0f0',
    'control-bg-active': '#e4e4e4',
    'control-border': '#cfcfcf',
    'control-border-strong': '#8e8e8e',
    'control-fg-quiet': '#5a5a5a',
    'accent-fg': '#ffffff',
```

and to `THEME.dark`:

```ts
    'control-bg': '#1c1c1c',
    'control-bg-hover': '#262626',
    'control-bg-active': '#303030',
    'control-border': '#3a3a3a',
    'control-border-strong': '#6e6e6e',
    'control-fg-quiet': '#a8a8a8',
    'accent-fg': '#0b1220',
```

and to `CONTRAST_PAIRS`:

```ts
  { fg: 'control-fg-quiet', bg: 'bg-pane', min: 4.5, note: 'quiet control labels' },
  { fg: 'control-border-strong', bg: 'bg-pane', min: 3, note: 'state boundaries' },
  { fg: 'accent-fg', bg: 'accent', min: 4.5, note: 'text on a filled primary' },
```

- [ ] **Step 2: Run it to see the new pairs measured**

Run: `npx vitest run src/lib/tokens.test.ts`
Expected: PASS with 6 new cases. If any fails, the hex is wrong — fix the hex, not the floor. Expected values: `control-fg-quiet` 6.61:1 / 7.61:1, `control-border-strong` 3.14:1 / 3.55:1, `accent-fg` on `accent` 5.17:1 / 7.37:1.

- [ ] **Step 3: Add the geometry tokens to `app.css`**

In the first `:root` block only — geometry does not vary by theme:

```css
  /* ── control geometry: one scale, no exceptions ──
     --control-h is WCAG 2.5.8's 24px minimum target. Do not raise it to
     "fix" a small control: this is a dense pro tool, and 24px is both the
     floor and the answer. */
  --control-h: 24px;
  --control-h-lg: 28px;
  --control-px: 8px;
  --control-px-lg: 12px;
  --control-gap: 6px;

  --radius-sm: 4px;
  --radius-md: 6px;
  /* RESERVED for aria-pressed toggles and the context meter. */
  --radius-pill: 999px;

  --control-font: 12px;
  --control-font-sm: 11px;

  --ring-w: 2px;
  --ring-offset: 1px;
```

- [ ] **Step 4: Add the control colours to all four blocks**

Light (`:root` and `:root[data-theme='light']`):

```css
  /* --border is 1.26:1 against --bg: it may SEPARATE things, but it can
     never be the only signal for a state. Use --control-border-strong
     (3.14:1) or --accent for anything that has to be read. */
  --control-bg: #ffffff;
  --control-bg-hover: #f0f0f0;
  --control-bg-active: #e4e4e4;
  --control-border: #cfcfcf;
  --control-border-strong: #8e8e8e;
  --control-fg: #1a1a1a;
  --control-fg-quiet: #5a5a5a;
  --accent-fg: #ffffff;
  --accent-soft: color-mix(in srgb, var(--accent) 10%, var(--bg));
  --ring: var(--accent);
```

Dark (the `prefers-color-scheme: dark` block and `:root[data-theme='dark']`):

```css
  --control-bg: #1c1c1c;
  --control-bg-hover: #262626;
  --control-bg-active: #303030;
  --control-border: #3a3a3a;
  --control-border-strong: #6e6e6e;
  --control-fg: #ededed;
  --control-fg-quiet: #a8a8a8;
  --accent-fg: #0b1220;
  --accent-soft: color-mix(in srgb, var(--accent) 16%, var(--bg));
  --ring: var(--accent);
```

- [ ] **Step 5: Write `src/lib/controls.css`**

```css
/**
 * The app's control primitives. Global on purpose: this is chrome, and
 * per-component scoped copies are exactly how the app arrived at four
 * different "primary button" conventions.
 *
 * Pill + border ⇒ clickable. Bare text ⇒ information (.tag). One rule.
 */

.btn {
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: var(--control-h);
  padding: 0 var(--control-px);
  /* Always present, transparent by default, so hover and selected states
     never shift layout. */
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--control-fg-quiet);
  font: inherit;
  font-size: var(--control-font);
  font-weight: 400;
  /* The box is set by `height`, never by the line box. */
  line-height: 1;
  white-space: nowrap;
  text-align: center;
  cursor: pointer;
  user-select: none;
  -webkit-user-select: none;
}

/* The only 2px ring in the app. Nothing else may use one. */
.btn:focus-visible {
  outline: var(--ring-w) solid var(--ring);
  outline-offset: var(--ring-offset);
}
/* Inside a clipping group, draw it inward instead. */
.btn-group .btn:focus-visible {
  outline-offset: calc(-1 * var(--ring-w));
}

/* One disabled convention. Never below 0.55: at 0.4, --fg-muted composites
   to ~1.75:1 and the reason for the disabling sits inside a control the
   user cannot see is there. */
.btn:disabled,
.btn[aria-disabled='true'] {
  opacity: 0.55;
  cursor: default;
}
.btn:disabled {
  pointer-events: none;
}

.btn--primary {
  height: var(--control-h-lg);
  padding: 0 var(--control-px-lg);
  border-color: var(--accent);
  background: var(--accent);
  color: var(--accent-fg);
  font-weight: 600;
}
.btn--primary:hover:not(:disabled) {
  background: color-mix(in srgb, #000 10%, var(--accent));
}
.btn--primary:active:not(:disabled) {
  background: color-mix(in srgb, #000 18%, var(--accent));
}

.btn--quiet {
  color: var(--control-fg-quiet);
}
/* The hover signal is a FILL, not a border: a --control-border change is
   1.56:1 and nobody sees it. */
.btn--quiet:hover:not(:disabled) {
  background: var(--control-bg-hover);
  color: var(--control-fg);
}
.btn--quiet:active:not(:disabled) {
  background: var(--control-bg-active);
}
/* A resting affordance, for controls that must look like controls before
   you hover them. */
.btn--quiet.is-bounded {
  border-color: var(--control-border);
  background: var(--control-bg);
}

.btn--toggle {
  border-radius: var(--radius-pill);
  border-color: var(--control-border);
  background: var(--control-bg);
  color: var(--control-fg-quiet);
}
.btn--toggle:hover:not(:disabled) {
  border-color: var(--control-border-strong);
  color: var(--control-fg);
}
/* Selected is carried by the boundary (4.95:1); the fill only reinforces
   it. A tint alone is ~1.2:1 and is decoration, not information. */
.btn--toggle[aria-pressed='true'],
.btn--toggle.is-active {
  border-color: var(--accent);
  background: var(--accent-soft);
  color: var(--control-fg);
  font-weight: 500;
}

/* Square by construction, so a close button cannot end up 13×13px. */
.btn--icon {
  width: var(--control-h);
  padding: 0;
  font-size: 14px;
  line-height: 1;
}
.btn--icon.btn--primary {
  width: var(--control-h-lg);
  height: var(--control-h-lg);
}

.btn--warn {
  --btn-tone: var(--usage-warn);
}
.btn--crit {
  --btn-tone: var(--usage-crit);
}
.btn--warn,
.btn--crit {
  color: var(--btn-tone);
  border-color: var(--btn-tone);
}
.btn--warn:hover:not(:disabled),
.btn--crit:hover:not(:disabled) {
  background: color-mix(in srgb, var(--btn-tone) 14%, transparent);
  color: var(--btn-tone);
}

/* Non-interactive information. No border, no pill, no box — this is what
   stops model/status pills impersonating buttons. */
.tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: var(--control-h);
  padding: 0;
  border: 0;
  background: none;
  color: var(--control-fg-quiet);
  font-size: var(--control-font-sm);
  white-space: nowrap;
  cursor: default;
}
.tag--mono {
  font-family: var(--mono);
}
```

- [ ] **Step 6: Import it**

`src/main.ts`, after the `app.css` import:

```ts
import './lib/controls.css';
```

- [ ] **Step 7: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS, unchanged test count. Nothing uses the primitives yet.

- [ ] **Step 8: Commit**

```bash
git add src/app.css src/lib/controls.css src/lib/tokens.ts src/lib/tokens.test.ts src/main.ts
git commit -m "feat(ui): a control token layer and four primitives"
```

---

### Task 6: One sticky bar — Find and turns move into the header

Today `.conv-header` and `.toolbar` stack to ≈58px of chrome with two hairlines ≈24px apart over surfaces that differ by 1.04:1. `.conv-header`'s `position: sticky` is a no-op — it is a direct flex child of `.conversation-panel` (`ConversationPanel.svelte:956`), which is a column flex, not a scroll container. Only `.toolbar`, inside `.scroller`, actually sticks. After this task the header sticks for the first time.

Two defects go with it: pressing ⌘F relocates the controls from the right edge to the left (`.toolbar` is `justify-content: flex-end`, `.find` stretches its input), and `N turns` vanishes while Find is open because `{#if findOpen} … {:else if conv}` are exclusive branches.

**Files:**
- Modify: `src/lib/ConversationHeader.svelte` (markup, props, `<style>`)
- Modify: `src/lib/ConversationPanel.svelte` (delete `.toolbar` / `.find` markup at 992–1036 and their CSS; pass the find and turns state down)
- Test: `src/lib/ConversationHeader.test.ts`, `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `.btn`, `.btn--quiet`, `.is-bounded`, `.btn--icon` from Task 5.
- Produces: `ConversationHeader` gains props
  `findOpen: boolean`, `findQuery: string`, `findCount: string`, `matchCount: number`,
  `turnEntries: TurnEntry[]`, `turnsOpen: boolean`,
  `onFindOpen: () => void`, `onFindClose: () => void`, `onFindInput: (q: string) => void`,
  `onFindKey: (e: KeyboardEvent) => void`, `onFindStep: (d: 1 | -1) => void`,
  `onTurnsToggle: () => void`, `onPickTurn: (rowKey: string) => void`,
  and exposes `focusFindInput(): void` so ⌘F still lands in the box.
  `TurnEntry` is the existing type behind `turnEntries` in `ConversationPanel`; export it from `src/lib/conversation_nav.ts` if it is not already exported.

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/ConversationHeader.test.ts`:

```ts
  const findProps = {
    findOpen: false, findQuery: '', findCount: '', matchCount: 0,
    turnEntries: [{ rowKey: 'r1', label: 'fix the bug', at: null }],
    turnsOpen: false,
    onFindOpen: vi.fn(), onFindClose: vi.fn(), onFindInput: vi.fn(),
    onFindKey: vi.fn(), onFindStep: vi.fn(), onTurnsToggle: vi.fn(), onPickTurn: vi.fn(),
  };

  it('carries the find and turns controls', () => {
    render(ConversationHeader, {
      session: session(), conversations: list, viewing: null, lastEvent: null,
      newerAvailable: false, onSelect: vi.fn(), ...findProps,
    });
    expect(screen.getByTestId('conv-find-button')).toBeTruthy();
    expect(screen.getByTestId('conv-turns-button').textContent).toContain('1 turn');
  });

  it('keeps the turns button reachable while find is open', () => {
    render(ConversationHeader, {
      session: session(), conversations: list, viewing: null, lastEvent: null,
      newerAvailable: false, onSelect: vi.fn(), ...findProps, findOpen: true, matchCount: 2, findCount: '1/2',
    });
    expect(screen.getByTestId('conv-find-input')).toBeTruthy();
    // The regression this task exists to prevent: today this is null.
    expect(screen.queryByTestId('conv-turns-button')).not.toBeNull();
    // The facts yield the middle slot to find, not the tool cluster.
    expect(screen.queryByTestId('conv-model')).toBeNull();
  });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `npx vitest run src/lib/ConversationHeader.test.ts`
Expected: FAIL — `Unable to find an element by: [data-testid="conv-find-button"]`

- [ ] **Step 3: Move the markup into the header**

In `ConversationHeader.svelte`, add the props above to the `$props()` destructure with their types, then replace the `<div class="facts">` block with three slots — switcher, middle (facts **or** find), and a tool cluster that is always present:

```svelte
  {#if findOpen}
    <div class="find-inline" data-testid="conv-find">
      <input
        class="field"
        type="search"
        data-testid="conv-find-input"
        aria-label="Find in conversation"
        placeholder="Find in conversation"
        bind:this={findInput}
        value={findQuery}
        oninput={(e) => onFindInput(e.currentTarget.value)}
        onkeydown={onFindKey}
      />
      <span class="tag find-count" data-testid="conv-find-count" aria-live="polite">{findCount}</span>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-prev" aria-label="Previous match" title="Previous match" disabled={matchCount === 0} onclick={() => onFindStep(-1)}>↑</button>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-next" aria-label="Next match" title="Next match" disabled={matchCount === 0} onclick={() => onFindStep(1)}>↓</button>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-close" aria-label="Close find" title="Close find" onclick={onFindClose}>×</button>
    </div>
  {:else}
    <div class="facts">
      <!-- meter, then model and status as .tag (Task 7), then last event -->
    </div>
  {/if}

  <div class="tools">
    <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-button" aria-label="Find in conversation" title="Find (⌘F / Ctrl+F)" onclick={onFindOpen}>⌕</button>
    {#if turnEntries.length > 0}
      <div class="turns-wrap" bind:this={turnsWrap}>
        <button type="button" class="btn btn--quiet" data-testid="conv-turns-button" aria-expanded={turnsOpen} bind:this={turnsButton} onclick={onTurnsToggle}
          >{turnEntries.length} turn{turnEntries.length === 1 ? '' : 's'}<span class="caret">▾</span></button>
        {#if turnsOpen}
          <!-- the existing .turn-index <ul>, moved verbatim, calling onPickTurn -->
        {/if}
      </div>
    {/if}
  </div>
```

Move the `.turn-index` list, its keyboard handler and its outside-pointerdown effect across from `ConversationPanel` unchanged. Export `focusFindInput` so ⌘F still focuses the box:

```ts
  let findInput: HTMLInputElement | undefined = $state();
  export function focusFindInput(): void {
    findInput?.focus();
    findInput?.select();
  }
```

- [ ] **Step 4: Delete the old bars and wire the header up**

In `ConversationPanel.svelte`, delete the `{#if findOpen} … {:else if conv} … {/if}` block at 992–1036 and the `.toolbar`, `.find`, `.find input`, `.find-count`, `.tb-btn`, `.turns-wrap` and `.turn-index` rules from its `<style>`. Keep every piece of state and every handler — they now flow down as props:

```svelte
  <ConversationHeader
    {session} {conversations} {viewing} {lastEvent} {newerAvailable} onSelect={select}
    bind:this={header}
    {findOpen} {findQuery} {findCount} matchCount={matches.length}
    {turnEntries} {turnsOpen}
    onFindOpen={() => void openFind()}
    onFindClose={closeFind}
    onFindInput={(q) => { findQuery = q; findIndex = 0; }}
    onFindKey={onFindKey}
    onFindStep={stepFind}
    onTurnsToggle={() => (turnsOpen = !turnsOpen)}
    onPickTurn={pickTurn}
  />
```

Change `openFind()` to call `header?.focusFindInput()` in place of its current direct `findInput` access, after `await tick()`.

- [ ] **Step 5: Style the single bar**

In `ConversationHeader.svelte`'s `<style>`, replace `.conv-header` and add the slots:

```css
  .conv-header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    min-height: 32px;
    /* One inset expression, shared with .thread and .composer (Task 8). */
    padding: 4px max(1.1rem, calc((100% - var(--chat-col)) / 2 + 1.1rem));
    background: var(--bg-pane);
    border-bottom: 1px solid var(--border);
  }
  .facts,
  .find-inline {
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    flex: 1 1 auto;
    min-width: 0;
  }
  /* Always at the right, find open or not: no pointer relocation on ⌘F. */
  .tools {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 0 0 auto;
  }
  .find-inline .field {
    flex: 1 1 auto;
    min-width: 0;
    max-width: 40ch;
    height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--fg);
    font: inherit;
    font-size: var(--control-font);
  }
  .find-inline .field:focus {
    border-color: var(--accent);
  }
  /* :focus-visible, not :focus — the global constraint reserves the 2px ring
     for it, and controls.css says so verbatim. */
  .find-inline .field:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: var(--ring-offset);
  }
  .find-count {
    min-width: 4.5ch;
    font-variant-numeric: tabular-nums;
  }
```

The `.conversation-panel` rule declares `--chat-col` on itself, and the header is its child, so the expression resolves.

- [ ] **Step 6: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS. `ConversationPanel.test.ts` cases that reach `conv-find-*` and `conv-turns-button` through `render(ConversationPanel, …)` keep working because the header is rendered inside it. If a case asserts on `conv-toolbar`, delete that assertion and say so in the commit — the element is gone by design.

- [ ] **Step 7: Commit**

```bash
git add src/lib/ConversationHeader.svelte src/lib/ConversationHeader.test.ts src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts src/lib/conversation_nav.ts
git commit -m "refactor(conversation): one sticky bar, and turns survive find"
```

---

### Task 7: Model and status stop impersonating buttons

`ConversationHeader`'s `.chip` and `ConversationPanel`'s `.chip` differ by 0.05rem of padding and are otherwise pixel-identical — but one is a `<span>` you cannot click and the other is a `<button>` that fires a prompt. That is the affordance failure the whole control system exists to fix.

The meter keeps its pill, because it is a meter. It takes the full colour for its border instead of `contextTint`'s ≈33% wash (≈1.3:1). **`contextTint` has three consumers, not one** — `ConversationHeader.svelte:161`, `SessionRowItem.svelte:372` and `SessionDetails.svelte:403` — so it is removed at all three or not at all.

**Files:**
- Modify: `src/lib/ConversationHeader.svelte` (markup + `<style>`)
- Modify: `src/lib/SessionRowItem.svelte:372`, `src/lib/SessionDetails.svelte:403`
- Modify: `src/lib/attention.ts` (delete `contextTint`)
- Test: `src/lib/attention.test.ts:114-115`, `src/lib/ConversationHeader.test.ts`

**Interfaces:**
- Consumes: `.tag`, `.tag--mono` from Task 5; `contextColor` from Task 1.
- Produces: `contextTint` no longer exists. Any later import of it is a compile error, which is the point.

- [ ] **Step 1: Write the failing tests**

In `src/lib/attention.test.ts`, delete lines 114–115 (the two `contextTint` assertions) and remove `contextTint` from the import at line 10.

Append to `src/lib/ConversationHeader.test.ts`:

```ts
  it('model and status are information, not controls', () => {
    render(ConversationHeader, {
      session: session(), conversations: list, viewing: null, lastEvent: null,
      newerAvailable: false, onSelect: vi.fn(), ...findProps,
    });
    const model = screen.getByTestId('conv-model');
    expect(model.tagName).toBe('SPAN');
    expect(model.className).toContain('tag');
    expect(model.className).not.toContain('chip');
    expect(screen.getByTestId('conv-status').className).toContain('tag');
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/attention.test.ts src/lib/ConversationHeader.test.ts`
Expected: FAIL — `expected 'chip' to contain 'tag'`

- [ ] **Step 3: Delete `contextTint` and repoint its three call sites**

Remove the `contextTint` function and its doc comment from `src/lib/attention.ts`. In each of the three components, change the meter's inline style from

```svelte
style="color: {contextColor(level)}; border-color: {contextTint(level)};"
```

to

```svelte
style="color: {contextColor(level)}; border-color: {contextColor(level)};"
```

and drop `contextTint` from each import. (`level` is `meter.level` in `ConversationHeader`, `ctxLevel` in the other two.)

- [ ] **Step 4: Convert the facts strip**

In `ConversationHeader.svelte`, inside `.facts`:

```svelte
      {#if model}<span class="tag tag--mono" data-testid="conv-model">{model}</span>{/if}
      {#if status}<span class="tag" data-testid="conv-status" data-status={status}>{status}</span>{/if}
      {#if lastEvent}<span class="tag last-event" data-testid="conv-last-event">{lastEvent}</span>{/if}
```

Replace the `.chip` rules in its `<style>` with:

```css
  .tag[data-status='compacting'],
  .tag[data-status='blocked'] {
    color: var(--usage-warn);
  }
  .tag[data-status='failed'] {
    color: var(--usage-crit);
  }
  .tag.last-event {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* A hairline divider instead of two competing pill borders. */
  .facts .tag + .tag::before {
    content: '';
    width: 1px;
    height: 11px;
    margin-right: var(--control-gap);
    background: var(--control-border);
  }
```

and update `.ctx` to the shared scale:

```css
  .ctx {
    position: relative;
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
    height: 18px;
    padding: 0 7px;
    border: 1px solid;
    border-radius: var(--radius-pill);
    overflow: hidden;
    font-size: var(--control-font-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .ctx-bar {
    position: absolute;
    inset: 0 auto 0 0;
    opacity: 0.22;
  }
```

Also delete the now-unused `.muted` rule if nothing else in the file uses it.

- [ ] **Step 5: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS. `svelte-check` is the gate that catches a missed `contextTint` import.

- [ ] **Step 6: Commit**

```bash
git add src/lib/attention.ts src/lib/attention.test.ts src/lib/ConversationHeader.svelte src/lib/ConversationHeader.test.ts src/lib/SessionRowItem.svelte src/lib/SessionDetails.svelte
git commit -m "refactor(ui): bordered pill means clickable, everywhere"
```

---

### Task 8: One inset, so the box lines up with the bubbles it makes

`.thread` applies `padding: 1rem 1.1rem` **inside** its 80ch box while `.composer-row` spans the full 80ch, and three different inset rules are in play across the header, the thread and the composer. (An earlier draft of this task claimed the two edges were 15.4px apart. They were not — that arithmetic assumes `border-box`, and this app has no global `box-sizing` reset. The task is a single-token cleanup, not a bug fix.)

**Files:**
- Modify: `src/lib/ConversationPanel.svelte` (`<style>`: `.thread`, `.composer`, `.chips`, `.composer-row`, `.slash-menu`, `.composer-error`, `.composer-status`)

**Interfaces:**
- Consumes: the header's inset expression from Task 6.
- Produces: `--chat-inset`, a variable on `.conversation-panel` that Tasks 9–12 and the attachments plan reuse.

- [ ] **Step 1: Declare the shared inset**

In `.conversation-panel`, next to `--chat-col`:

```css
    /* The one horizontal inset: the header, the turns and the composer all
       start on this edge. Before this existed, the textarea sat 15px left
       of the bubbles it produced. */
    --chat-inset: max(1.1rem, calc((100% - var(--chat-col)) / 2 + 1.1rem));
```

- [ ] **Step 2: Use it**

Replace the horizontal padding of `.thread` and `.composer` with `var(--chat-inset)`, and drop `max-width: var(--chat-col); margin: 0 auto` from `.chips`, `.composer-row`, `.slash-menu`, `.composer-error` and `.composer-status` — they now inherit the column from `.composer`'s padding. In `ConversationHeader.svelte`, replace the literal expression added in Task 6 with `padding: 4px var(--chat-inset);`.

- [ ] **Step 3: Run the full suite**

Run: `npx vitest run`
Expected: PASS, unchanged count. jsdom does not lay out, so this is a visual change with no assertion — the gate is that nothing breaks.

- [ ] **Step 4: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationHeader.svelte
git commit -m "style(conversation): one inset for the bar, the turns and the box"
```

---

### Task 9: The composer shell

The shell owns the border; the textarea is borderless inside it; Send becomes a 28px icon in the bottom-right. This is what makes attachments possible: today `.composer-row` is a flex row with a textarea and a sibling button, and there is nowhere a thumbnail could live that is visually inside the input.

The 103-character placeholder goes: at 12px in a 400px pane you see about a third of it, and it vanishes the moment you type.

**Files:**
- Modify: `src/lib/ConversationPanel.svelte` (composer markup ~1276–1296 and `<style>`)
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `.btn`, `.btn--icon`, `.btn--primary` from Task 5; `--chat-inset` from Task 8.
- Produces: `.composer-shell` — the element the attachments plan mounts its strip, drop veil and attach button into. It is `position: relative`.

- [ ] **Step 1: Write the failing test**

Append to `src/lib/ConversationPanel.test.ts` (inside the composer describe block, following the file's existing render helper):

```ts
  it('send is an icon button inside the shell and still submits', async () => {
    await renderPanel();
    const send = screen.getByTestId('conv-composer-send');
    expect(send.getAttribute('aria-label')).toBe('Send prompt');
    expect(send.closest('.composer-shell')).not.toBeNull();
    expect(screen.getByTestId('conv-composer-input').getAttribute('placeholder')).toBe('Send a prompt…');
  });
```

Use whatever the file's existing helper for mounting the panel with a promptable session is called; do not introduce a second one.

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/ConversationPanel.test.ts -t 'icon button inside the shell'`
Expected: FAIL — `expected null not to be null`

- [ ] **Step 3: Restructure the composer**

Replace `.composer-row` with the shell:

```svelte
      <div class="composer-shell">
        <textarea
          class="composer-input"
          data-testid="conv-composer-input"
          aria-label="Prompt"
          aria-controls={slashOpen ? SLASH_LIST_ID : undefined}
          aria-activedescendant={slashOpen ? slashOptionId(Math.min(slashIndex, slashMatches.length - 1)) : undefined}
          bind:this={box}
          bind:value={draft}
          oninput={onComposerInput}
          onkeydown={onComposerKey}
          rows="2"
          use:autoGrow={draft}
          placeholder="Send a prompt…"
          disabled={sending || viewing !== null}
        ></textarea>
        <div class="composer-actions">
          <span class="composer-hint" aria-hidden="true">↵ send · ⇧↵ newline · ↑ history</span>
          <button
            type="submit"
            class="btn btn--icon btn--primary"
            data-testid="conv-composer-send"
            aria-label="Send prompt"
            title="Send (Enter)"
            aria-keyshortcuts="Enter"
            disabled={!canSend}>{sending ? '…' : '↑'}</button>
        </div>
      </div>
```

- [ ] **Step 4: Style it**

Replace `.composer-row`, `.composer textarea` and `.composer button` with:

```css
  .composer-shell {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 6px 4px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    background: var(--control-bg);
  }
  .composer-shell:focus-within {
    border-color: var(--accent);
  }
  .composer-input {
    min-height: 40px;
    max-height: 168px;
    /* The box sizes itself to the draft (see autoGrow); a manual drag would
       only be overwritten on the next keystroke. */
    resize: none;
    padding: 2px 4px;
    border: 0;
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: 13px;
    line-height: 1.45;
  }
  .composer-input:focus {
    outline: none;
  }
  .composer-actions {
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    min-height: var(--control-h-lg);
  }
  .composer-hint {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--control-fg-quiet);
    font-size: var(--control-font-sm);
  }
  @container chat (max-width: 26rem) {
    .composer-hint { display: none; }
  }
```

The `chat` container is already declared on `.conversation-panel`.

- [ ] **Step 5: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS. Any case asserting the send button's text content `'Send'` or `'Sending…'` must move to `aria-label` / `title`; note it in the commit.

- [ ] **Step 6: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): one shell, with send inside it"
```

---

### Task 10: Chips hold one row, and `More` opens the rest

A wrapping chip row changes the composer's height as the preset list changes. One row, with what does not fit behind a trailing `More ▾`.

The measurement is extracted into a pure function so it is testable: jsdom reports `scrollWidth` and `clientWidth` as 0, so a component test can drive the state directly while a unit test covers the decision.

**Files:**
- Create: `src/lib/composer_overflow.ts`
- Create: `src/lib/composer_overflow.test.ts`
- Modify: `src/lib/ConversationPanel.svelte`
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `.btn`, `.btn--toggle` from Task 5.
- Produces: `needsMore(scrollWidth: number, clientWidth: number): boolean` and `OVERFLOW_SLACK: number` from `src/lib/composer_overflow.ts`.

- [ ] **Step 1: Write the failing unit test**

Create `src/lib/composer_overflow.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { needsMore, OVERFLOW_SLACK } from './composer_overflow';

describe('needsMore', () => {
  it('is false when the chips fit', () => {
    expect(needsMore(300, 300)).toBe(false);
    expect(needsMore(280, 300)).toBe(false);
  });
  it('ignores sub-pixel rounding', () => {
    expect(needsMore(300 + OVERFLOW_SLACK, 300)).toBe(false);
  });
  it('is true when they do not fit', () => {
    expect(needsMore(420, 300)).toBe(true);
  });
  it('treats an unmeasured row as fitting', () => {
    expect(needsMore(0, 0)).toBe(false);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/composer_overflow.test.ts`
Expected: FAIL — `Failed to resolve import "./composer_overflow"`

- [ ] **Step 3: Write the module**

```ts
/**
 * Whether the composer's chip row has more chips than fit on one line.
 *
 * Extracted from the component because jsdom lays nothing out — scrollWidth
 * and clientWidth are both 0 there — so the decision is unit-tested here and
 * the component is tested by driving its state.
 */

/** Sub-pixel rounding: a row is not "overflowing" by one pixel. */
export const OVERFLOW_SLACK = 1;

export function needsMore(scrollWidth: number, clientWidth: number): boolean {
  if (clientWidth === 0) return false;
  return scrollWidth > clientWidth + OVERFLOW_SLACK;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/lib/composer_overflow.test.ts`
Expected: PASS, 4 cases.

- [ ] **Step 5: Write the failing component test**

Append to `src/lib/ConversationPanel.test.ts`:

```ts
  it('collapses overflowing chips behind More and expands them', async () => {
    await renderPanel();
    const row = screen.getByTestId('conv-chips');
    // jsdom lays nothing out, so state the overflow the way the observer would.
    Object.defineProperty(row, 'scrollWidth', { value: 500, configurable: true });
    Object.defineProperty(row, 'clientWidth', { value: 300, configurable: true });
    window.dispatchEvent(new Event('resize'));
    await tick();

    const more = screen.getByTestId('conv-chips-more');
    expect(more.getAttribute('aria-expanded')).toBe('false');
    expect(row.getAttribute('data-expanded')).toBe('false');
    await fireEvent.click(more);
    expect(more.getAttribute('aria-expanded')).toBe('true');
    expect(row.getAttribute('data-expanded')).toBe('true');
  });
```

- [ ] **Step 6: Implement it in the component**

Add state and a measurement effect:

```ts
  import { needsMore } from './composer_overflow';

  let chipsRow: HTMLDivElement | undefined = $state();
  let chipsOverflow = $state(false);
  let chipsExpanded = $state(false);

  function measureChips() {
    if (!chipsRow) return;
    chipsOverflow = needsMore(chipsRow.scrollWidth, chipsRow.clientWidth);
    if (!chipsOverflow) chipsExpanded = false;
  }

  $effect(() => {
    if (!chipsRow) return;
    measureChips();
    // ResizeObserver is absent in jsdom; the resize listener is what the
    // component test drives, and both paths call the same measurement.
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measureChips);
    ro?.observe(chipsRow);
    window.addEventListener('resize', measureChips);
    return () => {
      ro?.disconnect();
      window.removeEventListener('resize', measureChips);
    };
  });
```

Markup — the row keeps `data-testid="conv-chips"` and every chip keeps `conv-chip`:

```svelte
      <div class="chips" data-testid="conv-chips" data-expanded={chipsExpanded} bind:this={chipsRow}>
        {#each $composerPresets as p, i (i)}
          {#if p.label.trim() && p.text.trim()}
            {@const suggested = suggestCompact && isCompactPreset(p)}
            <button
              type="button"
              class="btn btn--toggle"
              class:suggest={suggested}
              data-testid="conv-chip"
              data-suggested={suggested || undefined}
              title={suggested
                ? `Context window is ${Math.round(session.context_pct ?? 0)}% used. Compacting frees space.\n\nClick fills the box; Shift+click sends now.`
                : `${p.text}\n\nClick fills the box; Shift+click sends now.`}
              disabled={sending || viewing !== null}
              onclick={(e) => usePreset(p, e.shiftKey)}>{p.label}</button>
          {/if}
        {/each}
      </div>
      {#if chipsOverflow}
        <button
          type="button"
          class="btn btn--toggle chips-more"
          data-testid="conv-chips-more"
          aria-expanded={chipsExpanded}
          onclick={() => (chipsExpanded = !chipsExpanded)}>{chipsExpanded ? 'Less' : 'More'} ▾</button>
      {/if}
```

CSS:

```css
  .chips {
    display: flex;
    gap: var(--control-gap);
    margin: 0 0 6px;
    /* One row by default; growth is a deliberate toggle, not a reflow. */
    flex-wrap: nowrap;
    overflow: hidden;
  }
  .chips[data-expanded='true'] {
    flex-wrap: wrap;
    overflow: visible;
  }
```

- [ ] **Step 7: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/lib/composer_overflow.ts src/lib/composer_overflow.test.ts src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): chips hold one row, More opens the rest"
```

---

### Task 11: A frozen session is louder than Send

`⏎ Press Enter` appears exactly when `liveStuck === 'press_enter'` — the session is waiting on a keypress and nothing is moving. It is not a fourth preset. It gets its own full-width row above the shell.

`.chip.suggest`'s `box-shadow: 0 0 0 2px …` goes at the same time: it is visually a focus ring, and a suggestion must not outshout a focus indicator.

**Files:**
- Modify: `src/lib/ConversationPanel.svelte`
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `.btn`, `.btn--toggle`, `.btn--warn` from Task 5.
- Produces: nothing new. `conv-chip-enter` keeps its testid and its behaviour.

- [ ] **Step 1: Write the failing test**

```ts
  it('the stuck prompt gets its own row, not a seat among the presets', async () => {
    await renderPanel({ stuck_kind: 'press_enter' });
    const enter = screen.getByTestId('conv-chip-enter');
    expect(enter.closest('[data-testid="conv-chips"]')).toBeNull();
    expect(enter.className).toContain('btn--warn');
  });

  it('a suggested chip is toned, not ringed', async () => {
    await renderPanel({ context_pct: 88 });
    const chip = screen.getAllByTestId('conv-chip').find((c) => c.dataset.suggested === 'true');
    expect(chip).toBeTruthy();
    expect(chip!.className).toContain('btn--warn');
  });
```

Pass the session overrides through the file's existing render helper.

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/ConversationPanel.test.ts -t 'own row'`
Expected: FAIL — `expected <div> not to be null` is inverted: the element is currently inside the chips row.

- [ ] **Step 3: Move it out and retone the suggestion**

Above the `.chips` row:

```svelte
      {#if liveStuck === 'press_enter'}
        <div class="stuck-row">
          <button
            type="button"
            class="btn btn--toggle btn--warn"
            data-testid="conv-chip-enter"
            title="The session is waiting on a key press. Sends a bare Enter."
            disabled={sending || viewing !== null}
            onclick={() => void sendText('')}>⏎ Press Enter</button>
        </div>
      {/if}
```

Change the suggested chip's class from `class:suggest={suggested}` to `class:btn--warn={suggested}` and delete the `.chip.suggest` rule entirely.

CSS:

```css
  .stuck-row {
    display: flex;
    margin: 0 0 6px;
  }
  .stuck-row .btn {
    width: 100%;
    justify-content: flex-start;
  }
```

- [ ] **Step 4: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): a frozen session outranks the send button"
```

---

### Task 12: The transcript holds still while the composer grows

`.composer` is `flex: 0 0 auto` at the bottom of a column flex and `.thread-area` is `flex: 1 1 auto`, so the composer growing by 50px shrinks the scroller by 50px and slides the transcript under the reader's eye. This is the helper the attachments plan depends on.

**Files:**
- Modify: `src/lib/ConversationPanel.svelte`
- Test: `src/lib/ConversationPanel.test.ts`

**Interfaces:**
- Consumes: `scroller`, `atBottom`, `scrollToBottom` — all already in the component.
- Produces: `preserveThread(mutate: () => void): void`. The attachments plan wraps every `add()` and `remove()` in it.

- [ ] **Step 1: Write the failing test**

```ts
  it('preserveThread keeps the viewport on the same content when the composer grows', async () => {
    await renderPanel();
    const scroller = screen.getByTestId('conv-scroller');
    Object.defineProperty(scroller, 'clientHeight', { value: 400, configurable: true });
    Object.defineProperty(scroller, 'scrollHeight', { value: 2000, configurable: true });
    scroller.scrollTop = 500;
    // Not pinned to the bottom: the correction must apply.
    await fireEvent.scroll(scroller);

    getPanel().preserveThread(() => {
      Object.defineProperty(scroller, 'clientHeight', { value: 350, configurable: true });
    });
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    expect(scroller.scrollTop).toBe(550);
  });
```

Expose the instance through the file's existing pattern for reaching component internals; if there is none, export `preserveThread` from the component and assert it through a small wrapper rather than inventing a second access pattern.

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/ConversationPanel.test.ts -t 'preserveThread'`
Expected: FAIL — `preserveThread is not a function`

- [ ] **Step 3: Implement the helper**

```ts
  /** Run a mutation that changes the composer's height without moving the
   *  transcript under the reader. The composer is flex: 0 0 auto at the
   *  bottom of a column flex, so it grows by taking from the scroller. */
  export function preserveThread(mutate: () => void): void {
    const el = scroller;
    if (!el) {
      mutate();
      return;
    }
    const wasAtBottom = atBottom;
    const before = el.clientHeight;
    mutate();
    requestAnimationFrame(() => {
      if (wasAtBottom) {
        el.scrollTop = el.scrollHeight;
        return;
      }
      el.scrollTop += before - el.clientHeight;
    });
  }
```

Wrap the `chipsExpanded` toggle from Task 10 in it:

```svelte
          onclick={() => preserveThread(() => (chipsExpanded = !chipsExpanded))}>
```

- [ ] **Step 4: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib/ConversationPanel.svelte src/lib/ConversationPanel.test.ts
git commit -m "feat(composer): the transcript holds still when the box grows"
```

---

### Task 13: `PromptComposer` gets the same Send as the composer

`PromptComposer`'s Send and `ConversationPanel`'s Send are the same action — send a prompt to a session — drawn today as outlined vs filled, radius 4 vs 6, `0.85rem/400` vs `0.8rem/600`. This task makes them one.

It also fixes the reason channel: `title={sendBlocked ?? ''}` puts a `title=""` on an enabled button and makes the blocking reason unreachable for keyboard and screen-reader users.

**Files:**
- Modify: `src/lib/PromptComposer.svelte` (markup + `<style>`)
- Test: `src/lib/PromptComposer.test.ts`

**Interfaces:**
- Consumes: `.btn`, `.btn--primary`, `.btn--quiet.is-bounded` from Task 5.
- Produces: nothing new. `composer-send` keeps its testid.

- [ ] **Step 1: Write the failing test**

Append to `src/lib/PromptComposer.test.ts`:

```ts
  it('states why send is blocked, in text, not only in a tooltip', () => {
    // Render with a hub status that blocks send_prompt, the way the file's
    // other hub-blocked case does.
    renderBlocked();
    const send = screen.getByTestId('composer-send');
    expect(send.getAttribute('aria-disabled')).toBe('true');
    const id = send.getAttribute('aria-describedby');
    expect(id).toBeTruthy();
    expect(document.getElementById(id!)?.textContent).toContain('hub');
  });
```

Reuse whatever the file already does to produce a blocked state; do not add a second mock shape.

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/PromptComposer.test.ts -t 'blocked'`
Expected: FAIL — `expected null to be 'true'`

- [ ] **Step 3: Convert the actions row**

```svelte
    <div class="actions">
      <button type="button" class="btn btn--quiet is-bounded" onclick={onClose}>Cancel</button>
      <button
        type="button"
        class="btn btn--primary"
        aria-disabled={!canSend}
        aria-describedby={sendBlocked ? 'composer-send-blocked' : undefined}
        onclick={canSend ? send : undefined}
        data-testid="composer-send"
      >{sending ? 'Sending…' : 'Send'}</button>
    </div>
    {#if sendBlocked}
      <p id="composer-send-blocked" class="blocked-reason" role="status">{sendBlocked}</p>
    {/if}
```

Delete the `.actions button`, `.actions button:disabled` and `.actions button.primary` rules, and add:

```css
  .actions { display: flex; gap: var(--control-gap); justify-content: flex-end; }
  .blocked-reason {
    margin: 0.35rem 0 0;
    color: var(--usage-warn);
    font-size: var(--control-font-sm);
    text-align: right;
  }
```

`aria-disabled` keeps the button focusable and hoverable, so the reason is reachable; `.btn[aria-disabled='true']` already supplies the dimming from Task 5 without `pointer-events: none`.

- [ ] **Step 4: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS. A case asserting `send.disabled === true` must become `aria-disabled`; note it in the commit, because the element genuinely changed from unfocusable to focusable, and that is the improvement.

- [ ] **Step 5: Commit**

```bash
git add src/lib/PromptComposer.svelte src/lib/PromptComposer.test.ts
git commit -m "refactor(ui): one Send, and a blocking reason you can reach"
```

---

### Task 14: The remaining shared controls

Four components and two dialogs still carry their own geometry. After this task the app has one primary convention, one disabled convention and one focus ring.

**Files:**
- Modify: `src/lib/SegmentedControl.svelte`, `src/lib/HostChips.svelte`, `src/lib/CopyButton.svelte`, `src/lib/TransferChip.svelte`, `src/lib/ImportDialog.svelte`, `src/lib/NewBgSessionDialog.svelte`
- Test: the existing tests for each

**Interfaces:**
- Consumes: every primitive from Task 5.
- Produces: nothing new.

- [ ] **Step 1: Write the failing test for the `!important` removal**

`NewBgSessionDialog.svelte:135-138` carries three `!important` declarations that exist only because its base `button` rule and its modifier have equal specificity. Append to that component's test file:

```ts
  it('the primary action needs no specificity escape hatch', async () => {
    const css = await import('node:fs').then((fs) =>
      fs.readFileSync('src/lib/NewBgSessionDialog.svelte', 'utf8'),
    );
    expect(css).not.toContain('!important');
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/lib/NewBgSessionDialog.test.ts`
Expected: FAIL — the file contains `!important`.

- [ ] **Step 3: Convert each component**

- `SegmentedControl.svelte`: wrap the group in `class="btn-group seg"`, make each option `class="btn btn--toggle"` with `border-radius: 0` inside the group and `aria-pressed` for the active state. Delete its own `outline: 2px solid var(--accent)` focus rule — `.btn-group .btn:focus-visible` supplies it, inset.
- `HostChips.svelte`: `.host-pick` becomes `class="btn btn--toggle tag--mono"`, `aria-pressed` for active. Delete the local `padding`, `font-size`, `border-radius` and `:disabled` rules.
- `CopyButton.svelte`: becomes `class="btn btn--icon btn--quiet"` with the glyph `⧉`, keeping its existing `aria-label={copied ? 'Copied' : label}` and `title`. Remove the `opacity: 0` reveal — a 16.7px target you must find by hovering becomes a 24px target that is always there.
- `TransferChip.svelte`: `class="btn btn--quiet is-bounded"`; delete its `padding: 0` and local radius.
- `ImportDialog.svelte`: `.primary` becomes `class="btn btn--primary"`; delete `color: white` and `border: 0`.
- `ConversationPanel.svelte`: the `.retry-btn` rule Task 3 added as a stopgap becomes `class="btn btn--quiet is-bounded"` on the `conv-retry` button; delete the local rule.
- `NewBgSessionDialog.svelte`: `.btn-primary` becomes `class="btn btn--primary"`; delete all three `!important` declarations and the rules that needed them.

- [ ] **Step 4: Run the full suite**

Run: `npx vitest run && npx svelte-check --tsconfig ./tsconfig.json`
Expected: PASS. Cases that assert on an `.active` class must move to `aria-pressed="true"`, which is the accessible signal and was missing before.

- [ ] **Step 5: Commit**

```bash
git add src/lib/SegmentedControl.svelte src/lib/HostChips.svelte src/lib/CopyButton.svelte src/lib/TransferChip.svelte src/lib/ImportDialog.svelte src/lib/NewBgSessionDialog.svelte src/lib/*.test.ts
git commit -m "refactor(ui): every shared control onto the primitives"
```

---

## Done when

- `npx vitest run` and `npx svelte-check --tsconfig ./tsconfig.json` both pass.
- `grep -rn '!important' src/lib/NewBgSessionDialog.svelte` returns nothing. (`Sidebar.svelte:1077` keeps its `opacity: 1 !important` — that one fights a hover-reveal cascade, not a button's specificity, and is a separate fix.)
- `grep -rn 'contextTint' src/` returns nothing.
- The conversation panel has exactly one sticky bar, and `N turns` is reachable while Find is open.
- `src/lib/tokens.test.ts` asserts every documented contrast pair in both themes.
