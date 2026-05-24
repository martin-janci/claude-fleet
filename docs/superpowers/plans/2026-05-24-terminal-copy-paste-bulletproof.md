# Bulletproof Terminal Copy/Paste + Context Menu — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make terminal copy/paste deterministic by routing through the native macOS clipboard and a grid-owned selection model, and add a right-click context menu that works in both tmux and plain shells.

**Architecture:** Replace `navigator.clipboard` (WKWebView permission tooltip + silent failures) with `tauri-plugin-clipboard-manager`. Replace `window.getSelection()` (wiped by the drain re-render) with selection state held in the component, highlighted by an absolutely-positioned overlay layer (same technique as the cursor), with copy text extracted from the `Screen.cells` buffer. Pure helpers (`Screen.selectionText`, selection geometry) are unit-tested; mouse/menu wiring is verified by `svelte-check` + build + manual test.

**Tech Stack:** Svelte 5 (runes), TypeScript, Tauri 2.11, Vitest/jsdom, Rust.

**Spec:** `docs/superpowers/specs/2026-05-24-terminal-copy-paste-bulletproof-design.md`

**Coordinate convention (read first):** `eventToCell` (`TerminalView.svelte:80`) returns **1-based** `{col,row}`. `Screen.cells[row][col]` and the cursor overlay are **0-based**. This plan stores selection in **0-based** cell coords — subtract 1 when converting from `eventToCell`.

---

## Task 1: Native clipboard plugin + frontend wrapper

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `package.json`
- Modify: `src-tauri/src/lib.rs:464`
- Modify: `src-tauri/capabilities/default.json`
- Create: `src/lib/clipboard_native.ts`
- Test: `src/lib/clipboard_native.test.ts`

- [ ] **Step 1: Add the Rust + JS dependencies**

In `src-tauri/Cargo.toml`, under the existing `tauri-plugin-opener = "2"` line, add:

```toml
tauri-plugin-clipboard-manager = "2"
```

Install the JS side (also writes `package.json`):

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3
pnpm add @tauri-apps/plugin-clipboard-manager
```

- [ ] **Step 2: Register the plugin**

In `src-tauri/src/lib.rs`, the builder currently has (line ~464):

```rust
        .plugin(tauri_plugin_opener::init())
```

Add the clipboard plugin directly after it:

```rust
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
```

- [ ] **Step 3: Grant clipboard permissions**

In `src-tauri/capabilities/default.json`, change the `permissions` array from:

```json
  "permissions": [
    "core:default",
    "opener:default"
  ]
```

to:

```json
  "permissions": [
    "core:default",
    "opener:default",
    "clipboard-manager:allow-read-text",
    "clipboard-manager:allow-write-text"
  ]
```

- [ ] **Step 4: Write the failing test for the wrapper**

Create `src/lib/clipboard_native.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

const readText = vi.fn();
const writeText = vi.fn();
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: (...a: unknown[]) => readText(...a),
  writeText: (...a: unknown[]) => writeText(...a),
}));

import { nativeReadText, nativeWriteText } from './clipboard_native';

describe('clipboard_native', () => {
  beforeEach(() => {
    readText.mockReset();
    writeText.mockReset();
  });

  it('nativeReadText returns Ok with the clipboard text', async () => {
    readText.mockResolvedValue('hello');
    const r = await nativeReadText();
    expect(r).toEqual({ ok: true, value: 'hello' });
  });

  it('nativeReadText returns Err when the plugin rejects', async () => {
    readText.mockRejectedValue(new Error('denied'));
    const r = await nativeReadText();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.message).toBe('denied');
  });

  it('nativeWriteText writes and returns Ok', async () => {
    writeText.mockResolvedValue(undefined);
    const r = await nativeWriteText('copied');
    expect(writeText).toHaveBeenCalledWith('copied');
    expect(r).toEqual({ ok: true, value: undefined });
  });

  it('nativeWriteText returns Err when the plugin rejects', async () => {
    writeText.mockRejectedValue(new Error('nope'));
    const r = await nativeWriteText('x');
    expect(r.ok).toBe(false);
  });
});
```

- [ ] **Step 5: Run the test to verify it fails**

Run: `npx vitest run src/lib/clipboard_native.test.ts`
Expected: FAIL — `Failed to resolve import "./clipboard_native"`.

- [ ] **Step 6: Implement the wrapper**

Create `src/lib/clipboard_native.ts`:

```ts
// Native clipboard access via tauri-plugin-clipboard-manager. Used instead of
// navigator.clipboard, which in WKWebView shows a permission tooltip on read
// and fails silently. Returns Result so callers surface errors instead of
// swallowing them.
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
import type { Result } from './result';

function toErr(raw: unknown): { code: string; message: string } {
  if (raw instanceof Error) return { code: 'E_CLIPBOARD', message: raw.message };
  return { code: 'E_CLIPBOARD', message: String(raw) };
}

export async function nativeReadText(): Promise<Result<string>> {
  try {
    return { ok: true, value: (await readText()) ?? '' };
  } catch (raw) {
    return { ok: false, error: toErr(raw) };
  }
}

export async function nativeWriteText(text: string): Promise<Result<void>> {
  try {
    await writeText(text);
    return { ok: true, value: undefined };
  } catch (raw) {
    return { ok: false, error: toErr(raw) };
  }
}
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `npx vitest run src/lib/clipboard_native.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 8: Commit**

```bash
git add src-tauri/Cargo.toml package.json pnpm-lock.yaml src-tauri/src/lib.rs src-tauri/capabilities/default.json src/lib/clipboard_native.ts src/lib/clipboard_native.test.ts
git commit -m "feat(terminal): native clipboard plugin + Result-returning wrapper"
```

---

## Task 2: `Screen.selectionText` — extract selected text from the buffer

**Files:**
- Modify: `src/lib/ansi.ts` (add method to `class Screen`)
- Test: `src/lib/ansi.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `src/lib/ansi.test.ts` (it already imports `Screen`; add the `selectionText` import if the file imports named symbols — otherwise `Screen` is enough):

```ts
describe('Screen.selectionText', () => {
  function withText(lines: string[]): Screen {
    const cols = Math.max(...lines.map((l) => l.length), 1);
    const s = new Screen(lines.length, cols);
    lines.forEach((line, r) => {
      for (let c = 0; c < line.length; c++) s.cells[r][c].ch = line[c];
    });
    return s;
  }

  it('returns a single cell', () => {
    const s = withText(['abc']);
    expect(s.selectionText({ row: 0, col: 1 }, { row: 0, col: 1 })).toBe('b');
  });

  it('returns a single-row range inclusive of both ends', () => {
    const s = withText(['hello world']);
    expect(s.selectionText({ row: 0, col: 0 }, { row: 0, col: 4 })).toBe('hello');
  });

  it('spans multiple rows: first row from col, middle rows full, last row to col', () => {
    const s = withText(['abcde', 'fghij', 'klmno']);
    // from (0,2) to (2,1): "cde" + "fghij" + "kl"
    expect(s.selectionText({ row: 0, col: 2 }, { row: 2, col: 1 })).toBe('cde\nfghij\nkl');
  });

  it('normalizes a reversed anchor/focus', () => {
    const s = withText(['abcde']);
    expect(s.selectionText({ row: 0, col: 4 }, { row: 0, col: 0 })).toBe('abcde');
  });

  it('trims trailing whitespace per line (padded cells)', () => {
    // Cells default to ' ', so selecting past the text yields padding.
    const s = withText(['hi', 'yo']);
    expect(s.selectionText({ row: 0, col: 0 }, { row: 1, col: 4 })).toBe('hi\nyo');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/lib/ansi.test.ts -t selectionText`
Expected: FAIL — `selectionText is not a function`.

- [ ] **Step 3: Implement `selectionText`**

In `src/lib/ansi.ts`, add this public method inside `class Screen` (e.g. just after the `mouseSgr` getters, before the constructor or alongside other methods). Define the param type too:

```ts
  /** Extract selected text from the buffer for an inclusive cell range.
   *  Anchor/focus may be in any order. First row runs from its column to EOL,
   *  middle rows are whole lines, the last row runs to its column. Trailing
   *  whitespace is trimmed per line (cells are space-padded to full width). */
  selectionText(a: { row: number; col: number }, b: { row: number; col: number }): string {
    // Order the two endpoints in reading order (row, then col).
    const before = a.row < b.row || (a.row === b.row && a.col <= b.col);
    const start = before ? a : b;
    const end = before ? b : a;
    const r0 = Math.max(0, Math.min(this.rows - 1, start.row));
    const r1 = Math.max(0, Math.min(this.rows - 1, end.row));
    const out: string[] = [];
    for (let r = r0; r <= r1; r++) {
      const colFrom = r === r0 ? start.col : 0;
      const colTo = r === r1 ? end.col : this.cols - 1; // inclusive
      const from = Math.max(0, colFrom);
      const to = Math.min(this.cols - 1, colTo);
      let line = '';
      for (let c = from; c <= to; c++) line += this.cells[r][c].ch || ' ';
      out.push(line.replace(/[ \t]+$/, ''));
    }
    return out.join('\n');
  }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/lib/ansi.test.ts -t selectionText`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/ansi.ts src/lib/ansi.test.ts
git commit -m "feat(terminal): Screen.selectionText extracts text from the cell buffer"
```

---

## Task 3: Selection geometry helpers (normalize + overlay rects)

**Files:**
- Create: `src/lib/selection.ts`
- Test: `src/lib/selection.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/lib/selection.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { normalizeSelection, selectionRects, type CellPos } from './selection';

describe('normalizeSelection', () => {
  it('orders endpoints in reading order', () => {
    const a: CellPos = { row: 2, col: 1 };
    const b: CellPos = { row: 0, col: 4 };
    expect(normalizeSelection(a, b)).toEqual({ start: b, end: a });
  });
  it('orders by col within the same row', () => {
    const a: CellPos = { row: 1, col: 5 };
    const b: CellPos = { row: 1, col: 2 };
    expect(normalizeSelection(a, b)).toEqual({ start: b, end: a });
  });
});

describe('selectionRects', () => {
  // cols=10, cellWidth=8, cellHeight=16, pad=4
  const cfg = { cols: 10, cw: 8, ch: 16, pad: 4 };

  it('single-row selection is one rect covering the inclusive cell span', () => {
    const rects = selectionRects({ row: 0, col: 2 }, { row: 0, col: 4 }, cfg.cols, cfg.cw, cfg.ch, cfg.pad);
    expect(rects).toEqual([{ left: 4 + 2 * 8, top: 4 + 0 * 16, width: 3 * 8, height: 16 }]);
  });

  it('multi-row selection: first to EOL, middle full width, last to col', () => {
    const rects = selectionRects({ row: 0, col: 7 }, { row: 2, col: 1 }, cfg.cols, cfg.cw, cfg.ch, cfg.pad);
    expect(rects).toEqual([
      { left: 4 + 7 * 8, top: 4, width: (10 - 7) * 8, height: 16 },     // row 0: col 7..EOL
      { left: 4, top: 4 + 16, width: 10 * 8, height: 16 },              // row 1: full
      { left: 4, top: 4 + 32, width: (1 + 1) * 8, height: 16 },         // row 2: col 0..1 inclusive
    ]);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/lib/selection.test.ts`
Expected: FAIL — cannot resolve `./selection`.

- [ ] **Step 3: Implement the helpers**

Create `src/lib/selection.ts`:

```ts
// Pure selection geometry for the terminal grid. 0-based cell coordinates.
// Kept DOM-free so it can be unit-tested directly.

export interface CellPos {
  row: number;
  col: number;
}

export interface OverlayRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Order two endpoints into reading order (row first, then col). */
export function normalizeSelection(a: CellPos, b: CellPos): { start: CellPos; end: CellPos } {
  const before = a.row < b.row || (a.row === b.row && a.col <= b.col);
  return before ? { start: a, end: b } : { start: b, end: a };
}

/** Build one overlay rect per selected row segment. Endpoints are inclusive.
 *  First row runs from its col to end-of-line, middle rows span the full width,
 *  the last row runs from col 0 to its col. `pad` is the grid's edge padding. */
export function selectionRects(
  a: CellPos,
  b: CellPos,
  cols: number,
  cellWidth: number,
  cellHeight: number,
  pad: number,
): OverlayRect[] {
  const { start, end } = normalizeSelection(a, b);
  const rects: OverlayRect[] = [];
  for (let r = start.row; r <= end.row; r++) {
    const from = r === start.row ? start.col : 0;
    const toInclusive = r === end.row ? end.col : cols - 1;
    rects.push({
      left: pad + from * cellWidth,
      top: pad + r * cellHeight,
      width: (toInclusive - from + 1) * cellWidth,
      height: cellHeight,
    });
  }
  return rects;
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/lib/selection.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/selection.ts src/lib/selection.test.ts
git commit -m "feat(terminal): pure selection geometry helpers"
```

---

## Task 4: Selection state + overlay render + plain-shell drag-select + native auto-copy

This task wires selection for the **mouse-reporting-OFF** path (plain shell) and removes the DOM-selection copy. Verified by `svelte-check`, full test run, and manual test (no unit test — DOM/mouse integration).

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Add imports**

After the existing `import { pointInRect } from './geometry';` line, add:

```ts
  import { normalizeSelection, selectionRects, type CellPos } from './terminal_selection';
  import { nativeWriteText, nativeReadText } from './clipboard_native';
```

- [ ] **Step 2: Add selection state**

Near the other `$state` declarations (around `let dragOver = $state(false);`), add:

```ts
  /** Selection endpoints in 0-based grid cells; null when nothing selected.
   *  Held in component state so the drain re-render can't wipe it (unlike the
   *  old window.getSelection() path). */
  let selAnchor: CellPos | null = $state(null);
  let selFocus: CellPos | null = $state(null);
  /** True while a drag-select is in progress (between mousedown and mouseup). */
  let selecting = false;
```

- [ ] **Step 3: Add the derived overlay rects**

Next to the `cursor` derived block, add:

```ts
  // Selection highlight rects. Touch renderVersion so it tracks resizes/redraws.
  const selRects = $derived.by(() => {
    void renderVersion;
    if (!selAnchor || !selFocus || cellWidth <= 0 || cellHeight <= 0) return [];
    return selectionRects(selAnchor, selFocus, lastCols, cellWidth, cellHeight, 4);
  });
```

- [ ] **Step 4: Add a copy helper**

Add near `sendPaste`:

```ts
  /** Copy the current selection to the native clipboard. No-op if empty. */
  async function copySelection() {
    if (!screen || !selAnchor || !selFocus) return;
    const text = screen.selectionText(selAnchor, selFocus);
    if (text === '') return;
    const r = await nativeWriteText(text);
    if (!r.ok) openError = `Copy failed: ${r.error.message}`;
  }

  /** Convert a 1-based eventToCell result to a 0-based grid cell. */
  function cellFromEvent(e: MouseEvent): CellPos {
    const { col, row } = eventToCell(e);
    return { row: row - 1, col: col - 1 };
  }

  function clearSelection() {
    selAnchor = null;
    selFocus = null;
  }
```

- [ ] **Step 5: Replace `onMousedown` to start a selection when mouse reporting is off**

The current `onMousedown` (`TerminalView.svelte:170`) returns early unless `screen.mouseEnabled`. Replace the whole function body's early-return region so that when mouse reporting is OFF we start a selection. Find:

```ts
  function onMousedown(e: MouseEvent) {
    // Option (Alt) held → let the browser do a native text selection instead
    // of forwarding the click to the app, so the user can copy while mouse
    // reporting is on.
    if (e.altKey) return;
    if (!ptyOpen || !screen || !screen.mouseEnabled) return;
```

Replace with:

```ts
  function onMousedown(e: MouseEvent) {
    if (!ptyOpen || !screen) return;
    // Left-button only for selection; other buttons fall through to app forwarding.
    if (e.button === 0 && !screen.mouseEnabled && !e.altKey) {
      // Plain shell: begin a local drag-selection.
      e.preventDefault();
      (e.currentTarget as HTMLElement | null)?.focus();
      const cell = cellFromEvent(e);
      selAnchor = cell;
      selFocus = cell;
      selecting = true;
      const handleMove = (ev: MouseEvent) => {
        if (!selecting) return;
        selFocus = cellFromEvent(ev);
      };
      const handleUp = () => {
        selecting = false;
        removeWindowListeners?.();
        // Only copy a real drag-selection; a plain click clears any selection.
        if (
          selAnchor && selFocus &&
          (selAnchor.row !== selFocus.row || selAnchor.col !== selFocus.col)
        ) {
          void copySelection();
        } else {
          clearSelection();
        }
      };
      window.addEventListener('mousemove', handleMove);
      window.addEventListener('mouseup', handleUp);
      removeWindowListeners = () => {
        window.removeEventListener('mousemove', handleMove);
        window.removeEventListener('mouseup', handleUp);
        removeWindowListeners = null;
      };
      return;
    }
    // Option held with mouse reporting on → native selection (handled in Task 5);
    // for now keep the old forward-to-app guard.
    if (e.altKey) return;
    if (!screen.mouseEnabled) return;
```

(The rest of the original `onMousedown` body — the `if (e.button > 2) return;` line onward — stays unchanged.)

- [ ] **Step 6: Remove the old DOM-selection auto-copy**

The `onGridMouseup` function (`TerminalView.svelte:234`) copies `window.getSelection()` via `navigator.clipboard`. Delete the entire `onGridMouseup` function and remove `onmouseup={onGridMouseup}` from the grid `<div>` (`TerminalView.svelte:700`). Selection copy now happens in the mouseup handler from Step 5.

- [ ] **Step 7: Clear selection on a fresh attach**

In `openTerm()`, right after `screen = new Screen(dim.rows, dim.cols);`, add:

```ts
    clearSelection();
```

- [ ] **Step 8: Render the selection overlay**

In the grid markup, directly before the `{#if cursor}` block (`TerminalView.svelte:714`), add:

```svelte
      {#each selRects as r (r.top + ':' + r.left)}
        <div
          class="selection"
          style="left:{r.left}px; top:{r.top}px; width:{r.width}px; height:{r.height}px"
          aria-hidden="true"
        ></div>
      {/each}
```

- [ ] **Step 9: Add the selection style**

In the `<style>` block near `.cursor`, add:

```css
  .selection {
    position: absolute;
    background: rgba(120, 170, 255, 0.35);
    pointer-events: none;
    z-index: 1;
  }
```

- [ ] **Step 10: Verify build + tests + manual**

```bash
npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TerminalView|selection|clipboard" || echo "no new errors in touched files"
npx vitest run
```
Expected: vitest all pass; no new svelte-check errors referencing the touched files (the pre-existing `NewSessionDialog.test.ts` error is unrelated).

Manual (note for the executor — requires the GUI): `pnpm tauri dev`, attach to a **plain shell** session (run `bash`/`zsh` with no tmux, no mouse app), drag to select → text highlights and stays highlighted even as output arrives; the selection is on the clipboard (paste elsewhere).

- [ ] **Step 11: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): grid-owned selection + overlay + native auto-copy (plain shell)"
```

---

## Task 5: Mouse-reporting-ON — deferred click-vs-drag + Option-forward

Adds selection in TUI apps (tmux/vim/Claude) without breaking app clicks. Verified by `svelte-check` + manual test.

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Add deferred-press state**

Near the mouse-forwarding state block, add:

```ts
  /** When mouse reporting is on, a left press is deferred until we know whether
   *  it becomes a drag (→ local selection) or a click (→ forward to the app).
   *  Holds the pending press cell + the raw event button until resolved. */
  let pendingPress: { cell: CellPos; cb: number; startX: number; startY: number } | null = null;
```

- [ ] **Step 2: Handle the mouse-reporting-ON left press in `onMousedown`**

In `onMousedown` (after the plain-shell block from Task 4, before the `if (e.altKey) return;` app-forward guard), insert the deferred branch:

```ts
    // Mouse reporting ON, left button, no Option → defer: drag becomes a local
    // selection, a click (no movement) forwards to the app.
    if (e.button === 0 && screen.mouseEnabled && !e.altKey) {
      e.preventDefault();
      (e.currentTarget as HTMLElement | null)?.focus();
      const cell = cellFromEvent(e);
      pendingPress = { cell, cb: 0, startX: e.clientX, startY: e.clientY };
      clearSelection();
      const DRAG_PX = 4;
      const handleMove = (ev: MouseEvent) => {
        if (!pendingPress) return;
        const moved =
          Math.abs(ev.clientX - pendingPress.startX) > DRAG_PX ||
          Math.abs(ev.clientY - pendingPress.startY) > DRAG_PX;
        if (moved && !selecting) {
          // Promote to a local selection.
          selecting = true;
          selAnchor = pendingPress.cell;
        }
        if (selecting) selFocus = cellFromEvent(ev);
      };
      const handleUp = (ev: MouseEvent) => {
        removeWindowListeners?.();
        if (selecting) {
          selecting = false;
          void copySelection();
        } else if (pendingPress) {
          // No drag → forward a real click (press + release) to the app.
          const c = cellFromEvent(ev);
          const sgr = screen!.mouseSgr;
          sendMouse(encodeMouse(0, c.col + 1, c.row + 1, false, sgr));
          sendMouse(encodeMouse(0, c.col + 1, c.row + 1, true, sgr));
        }
        pendingPress = null;
      };
      window.addEventListener('mousemove', handleMove);
      window.addEventListener('mouseup', handleUp);
      removeWindowListeners = () => {
        window.removeEventListener('mousemove', handleMove);
        window.removeEventListener('mouseup', handleUp);
        removeWindowListeners = null;
      };
      return;
    }
```

Note: `encodeMouse` is already imported (`./ansi`) and expects 1-based col/row (hence `c.col + 1`).

- [ ] **Step 3: Confirm Option-forward still works**

The remaining original `onMousedown` tail handles `e.altKey` → falls through the early `if (e.altKey) return;`? No — verify: with Option held, the two new left-button branches are skipped (both require `!e.altKey`), so control reaches `if (e.altKey) return;`. That returns BEFORE forwarding, which means Option currently suppresses app forwarding (old behavior was Option=native browser select). We want **Option = forward to app**. Change that guard: find

```ts
    if (e.altKey) return;
    if (!screen.mouseEnabled) return;
```

(the lines that remain after the Task 4 edit) and replace with:

```ts
    if (!screen.mouseEnabled) return; // no reporting + not a left-select → ignore
    // Option+anything, or non-left buttons, fall through to app forwarding below.
```

So the original forwarding code (`if (e.button > 2) return;` onward) now runs for Option-held drags and for middle/right buttons, forwarding them to the app.

- [ ] **Step 4: Verify build + manual**

```bash
npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TerminalView" || echo "no new errors"
npx vitest run
```
Expected: pass / no new errors.

Manual (GUI): in a tmux session — a plain click still reaches tmux (e.g. selects a pane/menu); a click-drag highlights text locally and copies on release; Option+drag drives tmux's own mouse selection.

- [ ] **Step 5: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): drag-selects vs click-forwards in mouse-reporting apps"
```

---

## Task 6: Keybindings — Cmd+C / Cmd+V (native) / Cmd+A; drop Ctrl+V hijack

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Replace the paste keybinding block**

In `onKeydown` (`TerminalView.svelte:524`), the current block intercepts Cmd+V **and** Ctrl+V:

```ts
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'v') {
      e.preventDefault();
      void navigator.clipboard.readText().then((t) => sendPaste(t)).catch(() => {});
      return;
    }
```

Replace with Cmd-only paste (native) plus Cmd+C copy and Cmd+A select-all:

```ts
    // Cmd+V → paste from the native clipboard (bracketed-paste framing in
    // sendPaste). Ctrl+V is intentionally NOT intercepted so ^V reaches the app.
    if (e.metaKey && !e.altKey && e.key.toLowerCase() === 'v') {
      e.preventDefault();
      void nativeReadText().then((r) => {
        if (r.ok) sendPaste(r.value);
        else openError = `Paste failed: ${r.error.message}`;
      });
      return;
    }
    // Cmd+C → copy the selection. No selection → no-op (let nothing happen;
    // Ctrl+C still sends SIGINT via keyToBytes).
    if (e.metaKey && !e.altKey && e.key.toLowerCase() === 'c') {
      if (selAnchor && selFocus) {
        e.preventDefault();
        void copySelection();
      }
      return;
    }
    // Cmd+A → select the whole viewport.
    if (e.metaKey && !e.altKey && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      selAnchor = { row: 0, col: 0 };
      selFocus = { row: lastRows - 1, col: lastCols - 1 };
      return;
    }
```

- [ ] **Step 2: Verify build + tests**

```bash
npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TerminalView" || echo "no new errors"
npx vitest run
```
Expected: pass / no new errors.

Manual (GUI): select text, Cmd+C, paste into another app → matches. Copy in another app, focus terminal, Cmd+V → pastes with NO permission tooltip. Ctrl+V in a shell prints `^V`. Cmd+A highlights the whole screen.

- [ ] **Step 3: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): Cmd+C/Cmd+V (native) + Cmd+A, stop hijacking Ctrl+V"
```

---

## Task 7: Right-click context menu

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Add context-menu state**

Near the selection state, add:

```ts
  /** Context-menu position (client px) or null when hidden. */
  let ctxMenu: { x: number; y: number } | null = $state(null);
```

- [ ] **Step 2: Add the handlers**

Add these functions near `copySelection`:

```ts
  function onContextMenu(e: MouseEvent) {
    if (!ptyOpen) return;
    e.preventDefault();
    // Clamp so the ~160×96px menu stays on screen.
    const x = Math.min(e.clientX, window.innerWidth - 170);
    const y = Math.min(e.clientY, window.innerHeight - 110);
    ctxMenu = { x, y };
  }

  function closeCtxMenu() {
    ctxMenu = null;
  }

  async function ctxCopy() {
    closeCtxMenu();
    await copySelection();
  }

  async function ctxPaste() {
    closeCtxMenu();
    const r = await nativeReadText();
    if (r.ok) sendPaste(r.value);
    else openError = `Paste failed: ${r.error.message}`;
  }

  function ctxSelectAll() {
    closeCtxMenu();
    selAnchor = { row: 0, col: 0 };
    selFocus = { row: lastRows - 1, col: lastCols - 1 };
  }
```

- [ ] **Step 3: Wire the contextmenu event + dismissal**

On the grid `<div>` (`TerminalView.svelte:689`), add the attribute alongside the existing handlers:

```svelte
      oncontextmenu={onContextMenu}
```

Then prevent right-click from *also* being forwarded to the app (otherwise a mouse-reporting app receives a button-2 press in addition to our menu). In `onMousedown`, right after the `if (!ptyOpen || !screen) return;` guard at the top, add:

```ts
    // Right-click is reserved for our context menu (handled by onContextMenu).
    if (e.button === 2) return;
```

- [ ] **Step 4: Render the menu**

Inside `.wrap`, after the closing `</div>` of `.grid` but before `{#if openError}`, add:

```svelte
    {#if ctxMenu}
      <!-- Backdrop closes the menu on any outside click. -->
      <div
        class="ctx-backdrop"
        onmousedown={closeCtxMenu}
        oncontextmenu={(e) => { e.preventDefault(); closeCtxMenu(); }}
        role="presentation"
      ></div>
      <div class="ctx-menu" style="left:{ctxMenu.x}px; top:{ctxMenu.y}px" data-testid="terminal-ctx-menu">
        <button onclick={ctxCopy} disabled={!selAnchor || !selFocus}>Copy</button>
        <button onclick={ctxPaste}>Paste</button>
        <button onclick={ctxSelectAll}>Select All</button>
      </div>
    {/if}
```

- [ ] **Step 5: Close the menu on Escape**

In `onKeydown`, at the very top after `if (e.isComposing) return;`, add:

```ts
    if (e.key === 'Escape' && ctxMenu) { ctxMenu = null; return; }
```

- [ ] **Step 6: Add menu styles**

In `<style>`, add:

```css
  .ctx-backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .ctx-menu {
    position: fixed;
    z-index: 31;
    min-width: 150px;
    background: #1c1c1c;
    border: 1px solid #3a3a3a;
    border-radius: 6px;
    padding: 4px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.4);
    display: flex;
    flex-direction: column;
  }
  .ctx-menu button {
    text-align: left;
    background: none;
    border: none;
    color: #e8e8e8;
    padding: 6px 10px;
    border-radius: 4px;
    font: inherit;
    cursor: pointer;
  }
  .ctx-menu button:hover:not(:disabled) { background: #2d6cdf; }
  .ctx-menu button:disabled { color: #666; cursor: default; }
```

- [ ] **Step 7: Verify build + tests**

```bash
npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TerminalView" || echo "no new errors"
npx vitest run
```
Expected: pass / no new errors.

Manual (GUI): right-click in both a tmux session and a plain shell → menu appears; Copy is disabled with no selection; Paste inserts clipboard; Select All highlights; outside-click and Escape dismiss.

- [ ] **Step 8: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): right-click context menu (copy/paste/select all)"
```

---

## Task 8: Route OSC 52 remote-copy through the native clipboard

The last `navigator.clipboard` user is the OSC 52 handler set in `openTerm()`.

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Replace the OSC 52 clipboard write**

In `openTerm()`, the handler is:

```ts
    screen.onClipboard = (text) => {
      void navigator.clipboard.writeText(text).catch(() => {});
    };
```

Replace with:

```ts
    screen.onClipboard = (text) => {
      void nativeWriteText(text).then((r) => {
        if (!r.ok) openError = `Clipboard write failed: ${r.error.message}`;
      });
    };
```

- [ ] **Step 2: Confirm no `navigator.clipboard` remains**

Run: `grep -n "navigator.clipboard" src/lib/TerminalView.svelte`
Expected: no output.

- [ ] **Step 3: Verify build + full suite**

```bash
npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TerminalView" || echo "no new errors"
npx vitest run
```
Expected: all tests pass; no new errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "feat(terminal): route OSC 52 remote-copy through native clipboard"
```

---

## Final verification

- [ ] **Run the whole frontend suite:** `npx vitest run` → all pass.
- [ ] **Type-check:** `npx svelte-check --tsconfig ./tsconfig.json` → only the pre-existing `NewSessionDialog.test.ts` error (verify it exists on `main` too; unrelated to this work).
- [ ] **Rust builds** (needs Tauri system libs; may only pass in the GUI/dev environment): `cd src-tauri && cargo build`.
- [ ] **Manual smoke (GUI, `pnpm tauri dev`):**
  - Plain shell: drag-select highlights + survives output + Cmd+C / right-click Copy works.
  - tmux/Claude: drag selects locally, plain click reaches the app, Option+drag drives the app's mouse.
  - Cmd+V and right-click Paste insert clipboard with **no permission tooltip**.
  - Ctrl+V reaches the app; Ctrl+C still interrupts.
