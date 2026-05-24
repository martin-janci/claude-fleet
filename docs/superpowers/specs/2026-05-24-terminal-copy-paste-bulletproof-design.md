# Terminal copy / paste — bulletproof + context menu

Date: 2026-05-24
Status: Approved — ready for implementation plan

## Goal

Make terminal copy/paste **deterministic** and add a **right-click context
menu** that works in both tmux sessions and plain (non-tmux) shells. The
existing copy/paste (shipped in
`2026-05-22-terminal-copy-paste-dragdrop-design.md`) is unreliable: it "shows
copying but copies nothing", shows an extra permission tooltip on paste, and
"doesn't work at all without tmux".

Drag-and-drop (file/image upload) already works after the Retina coordinate fix
and is **out of scope** here.

Non-goals: scrollback selection (viewport only), inline-image paste, search,
selection that survives a resize.

## Context / current state

- **Copy today** is *copy-on-select only*: `onGridMouseup` reads
  `window.getSelection()` over the rendered `<span>`s and writes it with
  `navigator.clipboard.writeText`. The grid re-renders on every drain tick
  (`renderVersion++` rebuilds `{#each visibleRows}`), so a live terminal wipes
  the DOM selection mid-drag or right after → empty/partial copy.
- **Paste today** (`onKeydown`): intercepts **both** `Cmd+V` and `Ctrl+V` →
  `navigator.clipboard.readText()`. `readText()` triggers WKWebView's native
  clipboard-permission tooltip; failures are swallowed by `.catch(() => {})`.
  Hijacking `Ctrl+V` also denies the app its own `^V`.
- **OSC 52** remote-copy (`screen.onClipboard`) also uses
  `navigator.clipboard.writeText`.
- **Selection in mouse-reporting apps**: `onMousedown` forwards the drag to the
  PTY whenever `screen.mouseEnabled`, so the user must hold **Option** to select.
- **No context menu** exists; right-click is forwarded to the PTY or inert.
- **Render primitives we build on**: `Screen.cells[row][col]` (row-major,
  `src/lib/ansi.ts`) holds every cell's char + style. The cursor is already an
  absolutely-positioned overlay `<div>` computed from `cellWidth`/`cellHeight`
  + the grid's 4px padding (`TerminalView.svelte` `cursor` derived). `eventToCell`
  maps a mouse event → `{col, row}`. Tauri plugins are registered in
  `src-tauri/src/lib.rs` (`tauri_plugin_opener::init()`); capabilities live in
  `src-tauri/capabilities/default.json`.

## 1. Native clipboard (the reliability fix)

Stop using the browser clipboard inside WKWebView; route read/write through the
native macOS clipboard in Rust.

- Add `tauri-plugin-clipboard-manager` to `src-tauri/Cargo.toml` and
  `@tauri-apps/plugin-clipboard-manager` to `package.json`.
- Register `.plugin(tauri_plugin_clipboard_manager::init())` in `lib.rs`.
- Grant `clipboard-manager:allow-read-text` and
  `clipboard-manager:allow-write-text` in `capabilities/default.json`.
- Frontend wrapper `src/lib/clipboard_native.ts` exposing
  `nativeWriteText(s)` / `nativeReadText()` that call the plugin and return a
  `Result` (`src/lib/result.ts` style) — no silent swallow.
- Replace **every** `navigator.clipboard.*` call: copy-on-select, the OSC 52
  `screen.onClipboard` handler, and the `Cmd+V` paste path.

This removes the paste tooltip and the silent "copied nothing", and behaves
identically in plain shell and tmux (it never depended on tmux).

## 2. Grid-owned selection model

Replace `window.getSelection()` with selection state the re-render cannot touch.

- State in `TerminalView.svelte`: `selAnchor: {row, col} | null` and
  `selFocus: {row, col} | null` as `$state`. A helper normalizes them into an
  ordered `{start, end}` (start ≤ end in reading order).
- New method `Screen.selectionText(start, end): string` in `ansi.ts` extracts
  text from `cells[][]`: from `start.col` on the first row, full intermediate
  rows, up to `end.col` on the last row; trailing whitespace trimmed per line
  (same semantics as `trimSelectionText`). Pure and unit-tested.
- **Render**: a new `.selection` overlay layer inside `.grid`, rendered like the
  cursor — one absolutely-positioned `<div>` per selected row segment
  (`left/top/width/height` from `cellWidth`/`cellHeight` + 4px padding),
  `pointer-events: none`, semi-transparent highlight. Driven by `selAnchor`/
  `selFocus` and `renderVersion`. The run/`rowCache` pipeline is untouched.
- Scope: visible viewport only (no scrollback).
- Selection is cleared on: a new selection start, `openTerm()`/resize, and a
  plain click that doesn't become a drag.

## 3. Mouse interaction

`eventToCell` gives the cell for every handler.

- **Mouse reporting OFF** (plain shell): `mousedown` sets `selAnchor` = cell and
  begins a drag; window `mousemove` updates `selFocus`; `mouseup` finalizes and
  auto-copies a non-empty selection via the native clipboard.
- **Mouse reporting ON** (tmux/vim/Claude): **defer** the press. On `mousedown`
  record the cell but do not forward yet. On the first `mousemove` past a
  one-cell threshold → start a local selection (do not forward). On `mouseup`
  with no movement → forward press **and** release to the PTY (a normal app
  click). Holding **Option** forces the gesture to the PTY for both press and
  drag (lets the user drag inside vim's own mouse mode).
- Auto-copy on select is retained (now reading from the buffer, not the DOM).

## 4. Context menu

- A custom positioned `<div class="ctx-menu">`, shown on the grid's
  `contextmenu` event (`preventDefault`), regardless of mouse-reporting mode.
- Items: **Copy** (disabled when selection is empty), **Paste**, **Select All**.
- Actions reuse the same functions as the keybindings (§5).
- Dismiss on outside `mousedown`, `Escape`, or after an item runs.
- Positioned at the event point, clamped to stay within the window.

## 5. Keybindings

In `onKeydown`:

- `Cmd+C` → copy current selection via native clipboard; **no selection → no-op**
  (does not preventDefault, nothing sent). `Ctrl+C` is unchanged and still sends
  SIGINT via `keyToBytes`.
- `Cmd+V` → paste via native clipboard (`nativeReadText` → `sendPaste`, which
  keeps `sanitizePaste` + bracketed-paste framing). **Remove the `Ctrl+V`
  branch** so `^V` reaches the app.
- `Cmd+A` → Select All (entire viewport) and show the highlight.

## 6. Error handling

`nativeReadText`/`nativeWriteText` return a `Result`. On error, surface a quiet,
transient one-line indicator in the terminal chrome (reuse the `openError`/banner
style) instead of `.catch(() => {})`. Paste-on-error pastes nothing and reports;
copy-on-error reports. Determinism means these are now rare and visible, not
silent.

## 7. Testing

- `Screen.selectionText`: single cell; multi-line span; reversed anchor/focus
  (end before start); trailing-space trim; empty selection → `''`.
- Cell-range → overlay-rect geometry helper: correct `left/top/width/height` for
  single-row and multi-row selections given `cellWidth`/`cellHeight`/padding.
- Keep existing `clipboard.ts` tests (`sanitizePaste`/`framePaste`/
  `trimSelectionText`).
- Mouse drag and the native clipboard plugin are integration-level; coverage is
  via the pure helpers they delegate to. `nativeReadText`/`nativeWriteText` are
  thin plugin wrappers.

## Files touched

- `src/lib/TerminalView.svelte` — selection state + overlay, mouse rewrite,
  context menu, keybindings, native-clipboard wiring.
- `src/lib/ansi.ts` — `Screen.selectionText`.
- `src/lib/clipboard_native.ts` (new) — native read/write wrappers + tests.
- `src/lib/ansi.test.ts` (or a new `selection.test.ts`) — selectionText tests.
- `src-tauri/Cargo.toml`, `package.json` — clipboard-manager plugin.
- `src-tauri/src/lib.rs` — plugin registration.
- `src-tauri/capabilities/default.json` — clipboard permissions.
