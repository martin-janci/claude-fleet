<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { unarchiveSession } from './tidy';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { selectedSession } from './selection';
  import { hostByAlias } from './hosts';
  import { Screen, rowToRuns, runsKey, runStyleCss, type Run } from './ansi';
  import { pointInRect } from './geometry';
  import { selectionRects, type CellPos } from './terminal_selection';
  import { nativeWriteText } from './clipboard_native';
  import { hintAnchor } from './hints';
  import { toIpcError } from './result';
  import { push, pushError } from './toasts';
  import { repairSession, hasNoPane, showFriendlyNames } from './sessions';
  import { displayName } from './attention';
  import { keyToBytes, detectMac } from './terminal_keys';
  import { createDrainLoop } from './terminal_drain';
  import { createTerminalClipboard, pathsToPasteText } from './terminal_clipboard';
  import { createMouseController } from './terminal_mouse';
  import TransferChip from './TransferChip.svelte';
  import { fitCells } from './terminal_size';
  import { ownsTheFleet } from './hub';

  // ─────────────────────────────────────────────────────────────────────
  // Terminal pane — minimal ANSI renderer.
  //
  // We do NOT use xterm.js. In our Tauri 2.11 + macOS WKWebView setup,
  // xterm's renderer silently fails to repaint after the first write for
  // reasons we couldn't isolate without devtools (see commit history on
  // branch `main` around 2026-05). Instead we maintain a virtual screen
  // buffer (`./ansi.ts`) and render it as styled `<div>` rows, which is
  // dirt-simple DOM that we can prove repaints. Tradeoffs:
  //   - No scrollback beyond what tmux's own scroll buffer can show with
  //     C-b [.
  //   - Keyboard input is forwarded as raw bytes via the xterm key table in
  //     `./terminal_keys.ts` (arrows/Home/End/Ins/Del/F-keys with modifiers,
  //     Ctrl chords, Alt/Option as an ESC prefix unless the layout composed
  //     a printable ASCII character under it).
  //   - Selection follows text-input conventions (`./terminal_mouse.ts` +
  //     `./terminal_selection.ts`): drag, double-click word, triple-click
  //     line, Shift+click extend; typing drops the highlight.
  // For our use case (tmux + claude TUI legibly visible in-app) these
  // limits are acceptable.
  // ─────────────────────────────────────────────────────────────────────

  let container: HTMLDivElement | undefined = $state(undefined);
  let measureCell: HTMLSpanElement | undefined = $state(undefined);
  /** Glyphs in the metrics probe. One character's shrink-to-fit width carries
   *  a sub-pixel rounding error, and that width now sizes every run box as
   *  well as the overlays, so measure a run of them and divide. */
  const MEASURE_CHARS = 20;
  const MEASURE_SAMPLE = 'M'.repeat(MEASURE_CHARS);
  let screen: Screen | null = null;
  /** Bumped after every screen.write() so the reactive view recomputes. */
  let renderVersion = $state(0);
  let resizeObserver: ResizeObserver | null = null;
  // The attached PTY's identity. BOTH parts are compared by the open/attach
  // guard: a tmux_name alone is ambiguous across hosts (default names are
  // project-derived, so host A and host B often run a same-named session),
  // and selecting the twin must reattach rather than silently keep showing
  // the other host's terminal.
  let currentSession: string | null = $state(null);
  let currentHost: string | null = $state(null);

  function isAttachedTo(sess: { tmux_name: string; host_alias: string } | null | undefined): boolean {
    // `screen` too: a half-open pane (a PTY whose Screen was torn down under
    // it) must never satisfy the guard, or selecting that session again would
    // skip the reopen and leave a blank grid that still swallows keystrokes.
    return (
      !!sess && screen !== null && sess.tmux_name === currentSession && sess.host_alias === currentHost
    );
  }

  /** Forward bytes to the PTY. A rejection (PTY gone, host dropped) used to be
   *  swallowed; now it surfaces once — the toast store dedupes repeats. */
  function writePty(data: string) {
    void invoke('pty_write', { args: { data } }).catch((e) => {
      pushError(toIpcError(e), 'Terminal input failed');
    });
  }
  /** Drop-overlay state: shown while a drag is over the grid, switched to a
   *  spinner during the upload. */
  let dragOver = $state(false);
  let uploading = $state(false);
  /** Uploads still running for the current attach. Counted, not a flag: two
   *  drops in a row would otherwise clear the overlay when the first finishes. */
  let uploadsInFlight = 0;
  /** Selection endpoints in 0-based grid cells; null when nothing selected.
   *  Held in component state so the drain re-render can't wipe it (unlike the
   *  old window.getSelection() path). */
  let selAnchor: CellPos | null = $state(null);
  let selFocus: CellPos | null = $state(null);
  let openError: string | null = $state(null);
  /** The hidden textarea that actually owns keyboard focus. WebKit only runs
   *  an input-method session on an editable element, so a dead key, the
   *  press-and-hold accent popup, the emoji picker and every CJK IME need a
   *  real editable target — the grid is a plain div and gets none of them.
   *  It rides the cursor cell so the candidate window opens where the text
   *  will appear. */
  let imeInput: HTMLTextAreaElement | undefined = $state(undefined);
  /** True between compositionstart and compositionend. */
  let composing = false;
  /** Set for one macrotask after compositionend: WebKit delivers the key that
   *  COMMITTED the composition as a keydown right after it, with isComposing
   *  already false. */
  let compositionJustEnded = false;
  /** Keyboard focus is on the terminal. Drives the cursor's look: a solid
   *  block when focused, a hollow outline when not — so it is always clear
   *  where typing will land, as with a text input's caret. */
  let focused = $state(false);
  /** Bumped on every keystroke / paste that reaches the PTY. The cursor
   *  element is keyed on it so its blink animation restarts from the visible
   *  phase — a caret that stays solid while you type and only blinks when
   *  idle, as in a text input. */
  let blinkEpoch = $state(0);
  /** Context-menu position (client px) or null when hidden. */
  let ctxMenu: { x: number; y: number } | null = $state(null);
  let ptyOpen = false;
  let lastCols = $state(0);
  let lastRows = $state(0);
  /** Bytes drained since this attach. The header shows this and nothing
   *  per-tick: a counter that moved on every poll rewrote the header text
   *  about four times a second on a terminal that was doing nothing. */
  let totalBytes = $state(0);
  /** Measured advance width of a single monospace cell, in px. We compute
   *  this once after mount from a sample <span>. Without a sane fallback
   *  the geometry calc would yield NaN and the view would never size.
   *  Reactive because the grid publishes it as `--cell-w`: every run's box
   *  is pinned to a multiple of it, so the glyphs and the overlays that sit
   *  on the col × cellWidth grid can't disagree. */
  let cellWidth = $state(0);
  let cellHeight = 0;
  let disconnected = $state(false);
  /** Self-healing: when the PTY dies (EOF / reader error — e.g. the ssh attach
   *  to a remote host dropped or its ControlMaster wedged), auto-reattach a
   *  bounded number of times with backoff before falling back to the manual
   *  reconnect banner. The budget resets on a fresh selection, a manual
   *  reconnect, or an attach that stayed up past HEALTHY_ATTACH_MS, so the cap
   *  only bites on a genuinely persistent failure rather than a one-off blip. */
  let autoReconnecting = $state(false);
  let reconnectAttempts = 0;
  let autoReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  /** When the current attach came up, or null while nothing is attached. */
  let attachedAt: number | null = null;
  const MAX_AUTO_RECONNECT = 3;
  const AUTO_RECONNECT_BASE_MS = 600;
  /** How long an attach has to stay up before it counts as healthy and the
   *  self-heal budget is restored. Longer than a failing attach survives
   *  (ssh connect + a login profile + a tmux error is well under a second). */
  const HEALTHY_ATTACH_MS = 10_000;

  /** What `pty_drain` returns (src-tauri/src/pty.rs `PtyDrainResult`).
   *  `eof` and `overflowed` are out-of-band flags — the reader thread's own
   *  state, never something inferred from `data`. */
  interface PtyDrainResult {
    data: string;
    bytes: number;
    eof: boolean;
    overflowed: boolean;
  }

  /** One terminal-failure toast per attach: a broken chunk usually repeats on
   *  every redraw, and the drain loop now survives it, so the user would
   *  otherwise never learn why the pane looks wrong. Reset by openTerm. */
  let reportedTerminalError = false;

  function reportTerminalError(error: unknown) {
    if (reportedTerminalError) return;
    reportedTerminalError = true;
    const detail = error instanceof Error ? error.message : String(error);
    push({ kind: 'error', message: `Terminal output could not be rendered: ${detail}` });
  }

  const drain = createDrainLoop({
    drainOnce,
    attached: () => !!screen && ptyOpen,
    onError: reportTerminalError,
  });
  const { bumpDrain } = drain;

  const { sendPaste, copySelection, pasteFromClipboard } = createTerminalClipboard({
    ptyOpen: () => ptyOpen,
    screen: () => screen,
    selAnchor: () => selAnchor,
    selFocus: () => selFocus,
    setOpenError: (message) => (openError = message),
    writePty,
    bumpDrain,
  });

  const mouse = createMouseController({
    ptyOpen: () => ptyOpen,
    screen: () => screen,
    container: () => container,
    lastCols: () => lastCols,
    lastRows: () => lastRows,
    cellWidth: () => cellWidth,
    cellHeight: () => cellHeight,
    selAnchor: () => selAnchor,
    selFocus: () => selFocus,
    setSelAnchor: (cell) => (selAnchor = cell),
    setSelFocus: (cell) => (selFocus = cell),
    clearSelection,
    copySelection,
    writePty,
    focusInput,
  });
  const { onWheel } = mouse;

  /** Mouse presses focus the proxy through the controller; note the modality
   *  first so the focus handler knows this was not keyboard navigation. */
  function onMousedown(e: MouseEvent) {
    pointerFocusPending = true;
    // Only the focus this press causes may consume the flag.
    queueMicrotask(() => (pointerFocusPending = false));
    mouse.onMousedown(e);
  }

  function onContextMenu(e: MouseEvent) {
    if (!ptyOpen) return;
    e.preventDefault();
    // Clamp so the ~160x96px menu stays on screen.
    const x = Math.max(0, Math.min(e.clientX, window.innerWidth - 170));
    const y = Math.max(0, Math.min(e.clientY, window.innerHeight - 110));
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
    await paste();
  }

  /** Paste from the native clipboard. Input replaces the selection in a text
   *  input; here it simply drops the highlight and restarts the caret blink. */
  async function paste() {
    clearSelection();
    blinkEpoch++;
    await pasteFromClipboard();
  }

  function ctxSelectAll() {
    closeCtxMenu();
    selAnchor = { row: 0, col: 0 };
    selFocus = { row: lastRows - 1, col: lastCols - 1 };
  }

  function clearSelection() {
    selAnchor = null;
    selFocus = null;
  }

  /** Is a drag-drop point inside the terminal grid? Tauri delivers the macOS
   *  drag position in logical points (see geometry.ts), the same space as
   *  getBoundingClientRect(), so we compare directly — no devicePixelRatio
   *  scaling, which previously halved the point on Retina and missed the grid. */
  function pointOverGrid(px: number, py: number): boolean {
    if (!container) return false;
    return pointInRect(px, py, container.getBoundingClientRect());
  }

  async function handleDrop(paths: string[]) {
    if (!ptyOpen || !currentSession || !currentHost || paths.length === 0) return;
    // An scp of a large file takes many seconds. Pin the upload to the attach
    // that started it: pasting on arrival regardless typed host A's paths into
    // whatever session was attached by then — a different Claude prompt, on a
    // machine where those paths don't exist.
    const target = { tmux_name: currentSession, host_alias: currentHost };
    const gen = openGeneration;
    uploadsInFlight += 1;
    uploading = true;
    try {
      const remote = await invoke<string[]>('upload_to_session', {
        args: { host_alias: target.host_alias, session_name: target.tmux_name, local_paths: paths },
      });
      if (remote.length === 0) return;
      if (gen === openGeneration && isAttachedTo(target)) {
        sendPaste(pathsToPasteText(remote));
      } else {
        push({
          kind: 'info',
          message: `Uploaded to ${target.host_alias}:${target.tmux_name}: ${remote.join(' ')}`,
        });
      }
    } catch (e) {
      // Same rule for the error: it belongs to the pane that asked for it.
      const message = `Upload failed: ${toIpcError(e).message}`;
      if (gen === openGeneration) openError = message;
      else push({ kind: 'error', message });
    } finally {
      // closeTerm already cleared the overlay for a pane that moved on — and a
      // newer upload may own it by now.
      uploadsInFlight = Math.max(0, uploadsInFlight - 1);
      if (gen === openGeneration) uploading = uploadsInFlight > 0;
    }
  }

  $effect(() => {
    const sess = $selectedSession;
    if (!sess) {
      void closeTerm();
      return;
    }
    if (isAttachedTo(sess)) return;
    void openTerm();
  });

  // When the container element first appears after a selection (Svelte 5
  // mounts the {#if} block lazily) and we're not yet attached, open the
  // PTY. Required because the first effect above can fire before the
  // <div bind:this> has populated `container`.
  $effect(() => {
    if (container && $selectedSession && !isAttachedTo($selectedSession)) {
      void openTerm();
    }
  });

  // Native (OS-level) drag-drop. HTML5 drop in WKWebView can't expose real
  // file paths, so we use Tauri's window event, which does. We only act on
  // drops that land over the grid.
  $effect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === 'enter' || p.type === 'over') {
          dragOver = pointOverGrid(p.position.x, p.position.y);
        } else if (p.type === 'leave') {
          dragOver = false;
        } else if (p.type === 'drop') {
          const over = pointOverGrid(p.position.x, p.position.y);
          dragOver = false;
          if (over) void handleDrop(p.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  /** Reentrancy guard. The two $effects above can both call openTerm() for
   *  the same selection in one flush; openTerm is async and its
   *  `currentSession` guard is only set after `await pty_open`, so a second
   *  call would otherwise run a full second open — leaking a ResizeObserver
   *  and a drain timer and double-opening the PTY. */
  let opening = false;
  /** Set when an open request arrives while one is already in flight.
   *  Dropping such a request stranded the new selection whenever the running
   *  open then failed: nothing re-triggers the $effects, so the pane sat on
   *  the previous session's error. Coalesced here and run from openTerm's
   *  `finally` — but only when that open cannot have attached the pane
   *  itself, since closeTerm nulling `currentSession` re-runs the effects
   *  during every open. */
  let reopenPending = false;
  /** Open generation. closeTerm() and onDestroy bump it; every open captures
   *  it and abandons itself after any await once it no longer matches — the
   *  selection can go away (deselect, the row killed) or the whole pane can be
   *  unmounted while `repair_session` probes a host over SSH, and the resumed
   *  open would otherwise attach a PTY nobody drains. */
  let openGeneration = 0;
  /** Set by onDestroy: after this, nothing may touch the PTY. */
  let destroyed = false;
  /** The post-attach resize hint, so closeTerm can cancel it. */
  let postAttachTimer: ReturnType<typeof setTimeout> | null = null;

  /** Has the open that captured `gen` been superseded — by a close, a destroy,
   *  a newer open, or the selection moving on? */
  function openIsStale(gen: number, target: { tmux_name: string; host_alias: string }): boolean {
    if (destroyed || gen !== openGeneration) return true;
    const sel = $selectedSession;
    return !sel || sel.tmux_name !== target.tmux_name || sel.host_alias !== target.host_alias;
  }

  async function openTerm(isAutoReconnect = false) {
    if (opening) {
      reopenPending = true;
      return;
    }
    const sess = $selectedSession;
    if (!sess) return;
    if (!container) return;
    const target = { tmux_name: sess.tmux_name, host_alias: sess.host_alias };
    opening = true;
    // A fresh attach may render fine: let it report a parser failure again.
    reportedTerminalError = false;
    /** Set when this open stood down as stale instead of running to a
     *  conclusion for `target`. The coalesced request then has to run even
     *  when it names the same session — leaving and coming straight back to
     *  one session is exactly the case that bails out. */
    let bailed = false;
    /** This open's generation, claimed below once closeTerm has bumped it. */
    let gen = 0;
    /** The post-await re-check, recording that we stood down so the `finally`
     *  can tell a stale exit from an open that really reached `target`. */
    const standDown = () => (bailed = openIsStale(gen, target));
    try {
      // A fresh open (new selection, manual reconnect, detach/reattach button)
      // starts with a clean self-heal budget; an auto-reconnect must preserve
      // the running attempt count so the cap can actually be reached.
      if (!isAutoReconnect) reconnectAttempts = 0;
      await closeTerm();
      // Claim the generation closeTerm() just bumped. Anything that closes or
      // destroys from here on bumps it again and this open stands down.
      gen = ++openGeneration;
      if (standDown()) return;
      openError = null;
      disconnected = false;
      await tick();
      if (standDown()) return;

      measureCellSize();
      const dim = computeDimensions();
      lastCols = dim.cols;
      lastRows = dim.rows;
      screen = new Screen(dim.rows, dim.cols);
      clearSelection();
      // Reset any in-progress drag state so a session switch can't leave it stale.
      mouse.reset();
      screen.onClipboard = (text) => {
        void nativeWriteText(text).then((r) => {
          if (!r.ok && !openIsStale(gen, target)) openError = `Clipboard write failed: ${r.error.message}`;
        });
      };
      renderVersion++;

      // A pane drag fires ResizeObserver every frame; resizing the screen
      // buffer (a full re-mark of every row) and sending pty_resize (a
      // SIGWINCH + tmux redraw over SSH) on each one floods the PTY. Debounce
      // on the trailing edge: only the settled size is applied, and it always
      // is — the last frame of a drag is never dropped.
      resizeObserver = new ResizeObserver(scheduleResize);
      resizeObserver.observe(container);

      // Automatic workspace check before attach. It only CREATES what is
      // confirmed missing: re-adds a deleted, unregistered worktree from its
      // existing branch, and starts a tmux session that is confirmed dead. It
      // never respawns a live pane (that would kill a running Claude just
      // because it was selected), never unregisters, adopts or rebranches —
      // those are Repair workspace only; we say so instead. A healthy session
      // costs one probe; orphans and background rows have nothing to check. An
      // offline host is left to the attach error.
      //
      // A paired desktop does not run it at all. The check is
      // `repair_session { explicit: false }`, which the hub client REFUSES on
      // purpose (`backend/verdicts.rs`): routing it would quietly become the
      // hub's always-explicit repair, which unregisters, adopts and
      // rebranches. There is no safe variant to route, so there is nothing to
      // ask for — attempting it anyway put an E_LOCAL_ONLY toast on every
      // attach of a project-backed session. Repair workspace is unaffected:
      // it passes `explicit: true` and routes.
      if (sess.project_id != null && !hasNoPane(sess) && ownsTheFleet()) {
        const rep = await repairSession(sess.id);
        if (standDown()) return;
        if (rep.ok) {
          const v = rep.value;
          const actions = v?.actions ?? [];
          if (actions.length > 0) {
            const branch = v?.branch_source ? ` [branch: ${v.branch_source}]` : '';
            push({ kind: 'info', message: `Repaired workspace for ${sess.tmux_name}: ${actions.join('; ')}${branch}` });
          }
          if (v?.needs_explicit_repair || v?.tmux_cwd_stale) {
            push({
              kind: 'info',
              message: `${sess.tmux_name} needs Repair workspace: ${(v.warnings ?? []).join('; ')}`,
            });
          }
        } else if (rep.error.code !== 'E_HOST_OFFLINE') {
          pushError(rep.error, 'Workspace check failed');
        }
      }

      try {
        await invoke('pty_open', {
          args: {
            session_name: sess.tmux_name,
            host_alias: sess.host_alias,
            cols: dim.cols,
            rows: dim.rows,
          },
        });
      } catch (e) {
        // Only the session that caused the failure may show it; a stale open's
        // error under another session's header is pure confusion.
        if (standDown()) return;
        if (isAutoReconnect) {
          // Keep backing off instead of stopping after one try: nothing else
          // would ever call scheduleAutoReconnect again (the drain loop never
          // started), so the budget — and with it the manual banner — was
          // unreachable.
          scheduleAutoReconnect(target.tmux_name, target.host_alias);
        } else {
          openError = `PTY error: ${toIpcError(e).message}`;
        }
        return;
      }
      if (standDown()) {
        // The attach landed after the pane let go of it. Nobody will drain it
        // and the backend keeps exactly one PTY, so close it — no newer open
        // can have taken over, they are serialized by `opening`.
        try {
          await invoke('pty_close');
        } catch {
          /* nothing to undo */
        }
        return;
      }
      currentSession = sess.tmux_name;
      currentHost = sess.host_alias;
      ptyOpen = true;
      attachedAt = Date.now();
      // Work graph M7: a person attaching is a touch — it un-archives the
      // session and keeps tidy-up off it for an hour. Not an automatic
      // reconnect. Best-effort: an older hub without the action refuses it.
      if (!isAutoReconnect) void unarchiveSession(sess.id).catch(() => {});

      // Start the adaptive drain loop. 30 ms (~33 Hz) is the floor when output
      // is flowing; it backs off to DRAIN_MAX_MS when the terminal is idle.
      drain.start();

      // Hint tmux to redraw at our exact size by re-sending the dimensions
      // once after attach. Defends against race where pty_open runs before
      // the slave-side process has set up SIGWINCH handling.
      postAttachTimer = setTimeout(() => {
        postAttachTimer = null;
        if (!ptyOpen) return;
        void invoke('pty_resize', { args: { cols: lastCols, rows: lastRows } }).catch(() => {});
      }, 150);
    } finally {
      opening = false;
      const pending = reopenPending;
      reopenPending = false;
      const sel = $selectedSession;
      // Re-run the coalesced request when this open cannot have served it: it
      // stood down as stale, or it was for another session. An open that ran
      // its course for `target` and merely failed at pty_open is NOT re-run —
      // that would retry a just-failed attach in a tight loop (the self-heal
      // backoff owns that case). Comparing the identities alone was not
      // enough: leaving a session and coming straight back while its workspace
      // probe is out makes both identities equal, and the pane was then left
      // unattached at 'measuring…' with nothing reactive left to fix it.
      if (
        pending &&
        sel &&
        !destroyed &&
        !isAttachedTo(sel) &&
        (bailed || sel.tmux_name !== target.tmux_name || sel.host_alias !== target.host_alias)
      ) {
        void openTerm();
      }
    }
  }

  const RESIZE_DEBOUNCE_MS = 50;
  /** Floor on the gap between two applied resizes. The debounce alone only
   *  coalesces frames closer together than its window: a slow or jerky drag
   *  (observer callbacks 60-70 ms apart, or a busy main thread) still sent one
   *  pty_resize — a SIGWINCH plus a full tmux redraw over SSH — per frame. */
  const RESIZE_MIN_INTERVAL_MS = 250;
  let resizeTimer: ReturnType<typeof setTimeout> | null = null;
  let lastResizeAt = 0;

  /** One ResizeObserver frame: (re)arm the trailing timer, never sooner than
   *  RESIZE_MIN_INTERVAL_MS after the last applied resize. The settled size is
   *  still never dropped — the last frame's timer always gets to run. */
  function scheduleResize() {
    if (resizeTimer !== null) clearTimeout(resizeTimer);
    const wait = Math.max(RESIZE_DEBOUNCE_MS, lastResizeAt + RESIZE_MIN_INTERVAL_MS - Date.now());
    resizeTimer = setTimeout(applyResize, wait);
  }

  function applyResize() {
    resizeTimer = null;
    if (!screen) return;
    const next = computeDimensions();
    if (next.cols === lastCols && next.rows === lastRows) return;
    lastCols = next.cols;
    lastRows = next.rows;
    screen.resize(next.rows, next.cols);
    renderVersion++;
    lastResizeAt = Date.now();
    if (ptyOpen) {
      void invoke('pty_resize', { args: { cols: next.cols, rows: next.rows } }).catch(() => {});
    }
  }

  function measureCellSize() {
    if (!measureCell) return;
    // Font metrics don't change between sessions — measure once and reuse
    // across every openTerm() / reconnect.
    if (cellWidth > 0 && cellHeight > 0) return;
    const rect = measureCell.getBoundingClientRect();
    // Fall back to a sensible default if measurement returns zero (jsdom).
    cellWidth = rect.width > 0 ? rect.width / MEASURE_CHARS : 7.8;
    cellHeight = rect.height > 0 ? rect.height : 16;
  }

  function computeDimensions(): { cols: number; rows: number } {
    if (!container) return { cols: 80, rows: 24 };
    const cw = cellWidth > 0 ? cellWidth : 7.8;
    const ch = cellHeight > 0 ? cellHeight : 16;
    // Subtract our own 4px padding (see CSS) from both sides.
    const w = Math.max(1, container.clientWidth - 8);
    const h = Math.max(1, container.clientHeight - 8);
    // MIN_COLS/MIN_ROWS are the same floor pty.rs clamps to — see terminal_size.ts.
    return fitCells(w, h, cw, ch);
  }

  /** Drain the PTY buffer once. Returns true if any bytes were consumed. */
  async function drainOnce(): Promise<boolean> {
    if (!screen || !ptyOpen) return false;
    // Capture the screen we're draining into. If the session is switched
    // (openTerm builds a new Screen) while this pty_drain is in flight, the
    // resolved bytes belong to the old PTY — discard them rather than write
    // stale output into the new screen.
    const drainingInto = screen;
    let result: PtyDrainResult;
    try {
      result = await invoke<PtyDrainResult>('pty_drain');
    } catch {
      return false;
    }
    if (screen !== drainingInto) return false;
    // The backend had to throw output away (the un-drained buffer hit its
    // cap). What is left resumes mid-sequence and has lost the DECSET modes
    // tmux sends once per attach — alt screen, mouse, bracketed paste, scroll
    // region — so the Screen can't be repaired from the stream. Drop it and
    // re-attach: a fresh attach re-sends all of that and redraws.
    if (result.overflowed) {
      // Re-attach as a self-heal, not as a fresh user-initiated open: the
      // budget must keep counting, or a session that overflows repeatedly
      // would reconnect for ever and never raise the banner.
      void openTerm(true);
      return false;
    }
    if (result.bytes > 0) {
      totalBytes += result.bytes;
      try {
        screen.write(result.data);
      } catch (e) {
        // The bytes are already consumed, so a parser bug must not take the
        // rest of the tick (query replies, the EOF handling below) with it —
        // and the loop keeps polling, so the next tmux redraw repairs it.
        console.error('[terminal] screen.write failed', e);
        reportTerminalError(e);
      }
      renderVersion++;
      // Answer any terminal queries (DSR cursor position, DA) the output
      // carried — the parser has no back-channel, so we forward its replies.
      const reply = screen.takeReplies();
      if (reply !== '') writePty(reply);
    }
    // The PTY is gone (e.g. the SSH child to a remote host died — now within
    // ~10s thanks to the ServerAlive keepalive in pty.rs, instead of hanging
    // silently forever). `eof` is the reader thread's own flag, NOT a search
    // for the `[cf]` line it also prints: output that merely contains that
    // text (this repo's pty.rs on screen, say) used to tear down a healthy
    // attach. It arrives once the last byte has been handed over, so it
    // normally comes with bytes === 0 — hence checked outside that branch.
    if (result.eof) {
      scheduleAutoReconnect();
      return result.bytes > 0;
    }
    // Restore the self-heal budget on PROOF of health — an attach that has
    // lived past HEALTHY_ATTACH_MS — never on "some output arrived". A doomed
    // attach also prints (a login profile, `can't find session`), and taking
    // that as healthy reset the count every cycle: the cap was never reached
    // and the pane said "reconnecting…" forever.
    if (
      reconnectAttempts > 0 &&
      attachedAt !== null &&
      Date.now() - attachedAt >= HEALTHY_ATTACH_MS
    ) {
      reconnectAttempts = 0;
    }
    return result.bytes > 0;
  }

  /** Self-healing reattach. Called when the reader thread reports the PTY
   *  died. Schedules a bounded, backed-off reattach for the still-selected
   *  session; once the budget is spent, surfaces the manual banner instead.
   *  The identity is a parameter because the caller may be an open whose
   *  `pty_open` failed — `currentSession`/`currentHost` are null by then. */
  function scheduleAutoReconnect(
    sessionAtSchedule: string | null = currentSession,
    hostAtSchedule: string | null = currentHost,
  ) {
    if (autoReconnectTimer !== null) return; // one already pending
    if (reconnectAttempts >= MAX_AUTO_RECONNECT) {
      autoReconnecting = false;
      disconnected = true; // give up → manual one-click recovery
      return;
    }
    reconnectAttempts += 1;
    autoReconnecting = true;
    const delay = AUTO_RECONNECT_BASE_MS * reconnectAttempts; // 0.6s, 1.2s, 1.8s
    autoReconnectTimer = setTimeout(() => {
      autoReconnectTimer = null;
      autoReconnecting = false;
      // Bail if the user switched away or detached while we waited.
      const sel = $selectedSession;
      if (!sel || sel.tmux_name !== sessionAtSchedule || sel.host_alias !== hostAtSchedule) return;
      void openTerm(true);
    }, delay);
  }

  async function reconnect() {
    disconnected = false;
    reconnectAttempts = 0; // manual click → fresh budget
    await closeTerm();
    await openTerm();
  }

  async function closeTerm() {
    // Invalidate every open in flight. Done FIRST and unconditionally: an
    // open suspended in `repair_session` or `pty_open` must stand down even
    // when there is nothing here to tear down yet.
    openGeneration += 1;
    // Cancel any pending self-heal first — a scheduled auto-reconnect for a
    // now-stale session must never fire after a detach or session switch.
    // (Done before the no-op guard below so a lingering timer is always
    // cleared, and kept conditional so we don't write $state needlessly.)
    if (postAttachTimer !== null) {
      clearTimeout(postAttachTimer);
      postAttachTimer = null;
    }
    if (autoReconnectTimer !== null) {
      clearTimeout(autoReconnectTimer);
      autoReconnectTimer = null;
    }
    if (autoReconnecting) autoReconnecting = false;

    // No-op when there's nothing to clean up. Without this guard the
    // mount-time effect fires closeTerm() against a fresh component,
    // unconditionally writes state ($state assignments), and Svelte 5's
    // reactivity scheduler treats the cascade as an effect-update loop.
    const hadAnything =
      screen !== null || ptyOpen || drain.pending() || resizeObserver !== null || resizeTimer !== null;
    if (!hadAnything) return;

    // Drop the context menu so a session switch can't leave the backdrop stuck.
    ctxMenu = null;
    // A composition half-typed into the pane that is going away must not be
    // flushed into the next session's PTY.
    composing = false;
    compositionJustEnded = false;
    if (imeInput) imeInput.value = '';
    // The overlay and the error belong to the pane that is going away; an
    // upload still in flight checks the generation before it touches either.
    uploadsInFlight = 0;
    uploading = false;
    openError = null;

    drain.stop();
    resizeObserver?.disconnect();
    resizeObserver = null;
    if (resizeTimer !== null) {
      clearTimeout(resizeTimer);
      resizeTimer = null;
    }
    lastResizeAt = 0;
    screen = null;
    attachedAt = null;
    lastCols = 0;
    lastRows = 0;
    totalBytes = 0;
    renderVersion++;
    if (ptyOpen) {
      ptyOpen = false;
      try {
        await invoke('pty_close');
      } catch {
        /* nothing to undo */
      }
    }
    currentSession = null;
    currentHost = null;
  }

  /** macOS: Cmd+C/V/A are the clipboard chords and Option is the ESC-prefix
   *  key. Elsewhere Ctrl+Shift+C/V do copy/paste (the Linux terminal
   *  convention) so plain Ctrl+C/V still reach the app as ^C / ^V. */
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);

  function onKeydown(e: KeyboardEvent) {
    if (!ptyOpen) return;
    // An input method owns this keystroke: the finished text arrives through
    // the proxy's composition/input events instead. WebKit reports the keys it
    // swallowed while composing as keyCode 229 / key `Process`.
    if (e.isComposing || composing || e.keyCode === 229 || e.key === 'Process') return;
    // The key that COMMITTED a composition is delivered after compositionend
    // with isComposing already false. Forwarding it as well would submit
    // Claude Code's prompt (Enter) or append a stray space to the word.
    if (compositionJustEnded && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      return;
    }
    if (e.key === 'Escape' && ctxMenu) { ctxMenu = null; return; }
    const k = e.key.toLowerCase();
    const cmdChord = e.metaKey && !e.altKey && !e.ctrlKey;
    const ctrlShiftChord = !isMac && e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
    // Paste from the native clipboard (bracketed-paste framing in sendPaste).
    // Plain Ctrl+V is intentionally NOT intercepted so ^V reaches the app.
    if ((cmdChord || ctrlShiftChord) && k === 'v') {
      e.preventDefault();
      void paste();
      return;
    }
    // Copy the selection. Cmd+C with no selection falls through to the
    // browser; Ctrl+Shift+C with no selection is swallowed (it is the copy
    // chord, not SIGINT — plain Ctrl+C still sends ^C via keyToBytes).
    if ((cmdChord || ctrlShiftChord) && k === 'c') {
      if (selAnchor && selFocus) {
        e.preventDefault();
        void copySelection();
      } else if (ctrlShiftChord) {
        e.preventDefault();
      }
      return;
    }
    // Cmd+A (Ctrl+Shift+A elsewhere) → select the whole viewport.
    if ((cmdChord || ctrlShiftChord) && k === 'a') {
      e.preventDefault();
      selAnchor = { row: 0, col: 0 };
      selFocus = { row: lastRows - 1, col: lastCols - 1 };
      return;
    }
    // macOS press-and-hold: holding a letter is supposed to open the accent
    // popup (`é`, `ē`, …) rather than repeat it, but AppKit only gets to decide
    // that if the repeating keydown is left unprevented — a prevented one never
    // reaches interpretKeyEvents:. So hand the repeats to the proxy: it inserts
    // them as ordinary input events (press-and-hold off) or delivers the accent
    // the user picked (press-and-hold on), either way through flushImeInput.
    if (isMac && e.repeat && !e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) return;
    const bytes = keyToBytes(e, { appCursor: screen?.appCursorKeys ?? false, isMac });
    if (bytes === null) return;
    e.preventDefault();
    writePty(bytes);
    // Typing into a text input collapses its selection; the highlight would
    // otherwise sit on stale cells while the screen redraws under it.
    clearSelection();
    blinkEpoch++;
    // The keystroke will produce output (echo / TUI redraw); pull the drain
    // loop back to full rate so it doesn't sit on a backed-off delay.
    bumpDrain();
  }

  /** Move keyboard focus to the IME proxy. Everything that used to focus the
   *  grid goes through here (its own onfocus included), so an input method
   *  always has an editable element to attach to. */
  /** Whether the current focus arrived from the keyboard. `:focus-visible`
   *  can't tell us: a UA always matches it on a focused text control, so the
   *  ring would appear on every click once focus moved to the proxy. */
  let keyboardFocus = $state(false);
  let pointerFocusPending = false;

  function onProxyFocus() {
    focused = true;
    keyboardFocus = !pointerFocusPending;
    pointerFocusPending = false;
  }

  /** A composition the browser never ends (focus pulled away mid-preedit)
   *  would otherwise leave `composing` true, and onKeydown swallows every
   *  keystroke while it is. */
  function onProxyBlur() {
    focused = false;
    keyboardFocus = false;
    composing = false;
    compositionJustEnded = false;
    if (imeInput) imeInput.value = '';
  }

  function focusInput() {
    (imeInput ?? container)?.focus({ preventScroll: true });
  }

  /** Send whatever the input method left in the proxy, and empty it.
   *
   *  Both compositionend and a non-composing input event call this, because
   *  WebKit and Chromium disagree about which of the two fires first and with
   *  what `data`. Reading the element's value rather than the event's payload
   *  makes the proxy the single source of truth: whichever event runs second
   *  finds it already empty and sends nothing, so the composed string can
   *  never go out twice. */
  function flushImeInput() {
    const el = imeInput;
    if (!el) return;
    const text = el.value;
    if (text) el.value = '';
    if (!text || !ptyOpen) return;
    writePty(text);
    clearSelection();
    blinkEpoch++;
    bumpDrain();
  }

  function onCompositionStart() {
    composing = true;
  }

  /** Forward IME / dead-key composed text (Slovak `á`, the press-and-hold
   *  accent popup, a CJK commit) — it never reaches `onKeydown` as a single
   *  printable char. */
  function onCompositionEnd() {
    composing = false;
    compositionJustEnded = true;
    // One macrotask is all the commit keydown gets; anything later is a real
    // keystroke the user meant to send.
    setTimeout(() => (compositionJustEnded = false), 0);
    flushImeInput();
  }

  /** Text inserted without a composition: the emoji picker, dictation, a
   *  paste the OS routed through the proxy. A printable keystroke never gets
   *  here — `onKeydown` calls preventDefault() for it, and a prevented
   *  keydown produces no input event. */
  function onImeInput(e: Event) {
    if (composing || (e as InputEvent).isComposing) return;
    const type = (e as InputEvent).inputType;
    if (type === 'insertFromPaste' || type === 'insertFromDrop') {
      // The macOS Edit ▸ Paste menu item and a drop onto the proxy land here,
      // not in our Cmd+V handler. Route them through the paste path so the
      // text is sanitised and bracketed like every other paste.
      const el = imeInput;
      const text = el ? el.value : '';
      if (el) el.value = '';
      if (text && ptyOpen) sendPaste(text);
      return;
    }
    flushImeInput();
  }


  onDestroy(() => {
    // Before closeTerm, so an open resuming from an await sees it at once.
    destroyed = true;
    openGeneration += 1;
    void closeTerm();
    mouse.dispose();
  });

  // Per-row render cache, keyed by the Screen instance so it resets on a
  // session switch. Each entry holds the row's `Screen.rowVersion` at build
  // time plus its derived `key` + `runs`.
  let rowCache: { ver: number; key: string; runs: Run[] }[] = [];
  let cacheScreen: Screen | null = null;

  // Derived view: a list of rows, each carrying its styled runs plus a
  // content-derived `key`. Reading `renderVersion` makes Svelte recompute
  // whenever screen.write() bumps it.
  //
  // The key (`runsKey`) encodes the row index followed by every run's style,
  // cell count and text. When a row's content changes, its key changes — and
  // so does the width of any run that moved — so Svelte destroys and
  // recreates that row's <div> instead of mutating its text nodes in place.
  // Recreating the DOM node is what forces WKWebView to repaint it: in-place
  // text mutation across many rows in one frame leaves some rows unpainted,
  // which shows up as "duplicated" lines/chars where content moved.
  //
  // A row whose `Screen.rowVersion` is unchanged since we last drew it reuses
  // its cached entry untouched, so an idle screen costs almost nothing and a
  // typical update only re-derives the few rows that actually moved.
  const visibleRows = $derived.by<{ key: string; runs: Run[] }[]>(() => {
    // Touch the version so the derived recomputes; also gate on screen.
    void renderVersion;
    const scr = screen;
    if (!scr) return [];
    if (cacheScreen !== scr) {
      rowCache = [];
      cacheScreen = scr;
    }
    const out: { key: string; runs: Run[] }[] = new Array(scr.rows);
    for (let r = 0; r < scr.rows; r++) {
      const ver = scr.rowVersion[r];
      const cached = rowCache[r];
      if (cached !== undefined && cached.ver === ver) {
        out[r] = cached;
        continue;
      }
      const runs = rowToRuns(scr.cells[r]);
      const entry = { ver, key: runsKey(r, runs), runs };
      rowCache[r] = entry;
      out[r] = entry;
    }
    if (rowCache.length > scr.rows) rowCache.length = scr.rows;
    return out;
  });

  // Memoized: a screen uses only a handful of distinct (fg, bg, attrs)
  // combos, but runStyle is called for every run on every render — caching
  // collapses it to a Map lookup.
  const styleCache = new Map<string, string>();

  // Cursor overlay position. Touch renderVersion so it tracks every drain.
  // Null when hidden (?25l) or before font metrics are measured. The grid has
  // 4px padding; cells run cellWidth × cellHeight from there.
  //
  // Shape and blink follow the app's DECSCUSR request (`CSI Ps SP q`):
  // 0/1 blinking block (the default), 2 steady block, 3/4 underline, 5/6
  // bar — so a bar-caret app (a shell with `cursor-shape`, an editor in
  // insert mode) looks the same here as in a real terminal. A cursor parked
  // past the last column (deferred wrap) is drawn on the last column, as
  // xterm does; a cursor on a wide glyph covers both of its cells.
  const cursor = $derived.by<{
    left: number; top: number; w: number; h: number;
    shape: 'block' | 'underline' | 'bar'; blink: boolean;
  } | null>(() => {
    void renderVersion;
    if (!screen || !screen.cursorVisible) return null;
    if (cellWidth <= 0 || cellHeight <= 0) return null;
    const col = Math.min(screen.cursorCol, screen.cols - 1);
    const row = screen.cells[screen.cursorRow];
    const wide = row !== undefined && row[col]?.ch !== '' && row[col + 1]?.ch === '';
    const st = screen.cursorStyle;
    return {
      left: 4 + col * cellWidth,
      top: 4 + screen.cursorRow * cellHeight,
      w: wide ? 2 * cellWidth : cellWidth,
      h: cellHeight,
      shape: st >= 5 ? 'bar' : st >= 3 ? 'underline' : 'block',
      blink: st === 0 || st === 1 || st === 3 || st === 5,
    };
  });

  // Where the caret IS, as opposed to where it is drawn. The IME proxy rides
  // this and not `cursor`, which is null whenever the app hid the cursor
  // (`CSI ?25l`) — the normal state for a full-screen TUI, Ink-based Claude
  // Code included. Anchoring the proxy to the overlay parked the candidate
  // window, the press-and-hold accent popup and the emoji picker in the
  // pane's top-left corner for exactly the app this terminal exists to run.
  const caretAt = $derived.by<{ left: number; top: number; h: number } | null>(() => {
    void renderVersion;
    if (!screen || cellWidth <= 0 || cellHeight <= 0) return null;
    return {
      left: 4 + Math.min(screen.cursorCol, screen.cols - 1) * cellWidth,
      top: 4 + screen.cursorRow * cellHeight,
      h: cellHeight,
    };
  });

  // Selection highlight rects. Touch renderVersion so it tracks resizes/redraws.
  const selRects = $derived.by(() => {
    void renderVersion;
    if (!selAnchor || !selFocus || cellWidth <= 0 || cellHeight <= 0) return [];
    // Pass the live cells: snapping to whole glyphs has to use the grid as it
    // is now, or a redraw under the selection leaves highlight and copy apart.
    return selectionRects(selAnchor, selFocus, lastCols, cellWidth, cellHeight, 4, screen?.cells);
  });

  function runStyle(run: Run): string {
    const cacheKey = `${run.fg}|${run.bg}|${run.attrs}`;
    let style = styleCache.get(cacheKey);
    if (style === undefined) {
      style = runStyleCss(run);
      styleCache.set(cacheKey, style);
    }
    return style;
  }

  // Whether the selected session's host is reachable over SSH at all.
  // `selectedSession` only carries `host_alias`, so look the row up; an
  // unknown host (not yet loaded) is treated as `ssh`, which means we try to
  // attach and let ssh itself say no, rather than refusing on a guess.
  const selectedSessionHostTransport = $derived(
    $selectedSession ? ($hostByAlias.get($selectedSession.host_alias)?.transport ?? 'ssh') : 'ssh',
  );
  // `transport: 'agent'` says the HUB cannot dial this host — that is why the
  // host dials out instead. It says nothing about THIS machine: `pty_open`
  // spawns `ssh <host>` against this machine's own ssh config, which may well
  // have a route (an entry with a ProxyCommand through a box that can reach
  // it). So the pane attaches every session alike and uses this only to
  // explain a failure that has already happened, never to refuse in advance.
  const selectedIsAgentHost = $derived(
    !!$selectedSession && selectedSessionHostTransport === 'agent',
  );
</script>

{#if $selectedSession}
  <div class="wrap">
    {#if autoReconnecting}
      <div class="reconnect-banner" data-testid="terminal-autoreconnect-banner">
        Connection lost — reconnecting…
      </div>
    {:else if disconnected}
      <div class="reconnect-banner" data-testid="terminal-reconnect-banner">
        Connection lost.
        <button onclick={reconnect}>Reconnect</button>
      </div>
    {/if}
    <div class="header" data-testid="terminal-header" use:hintAnchor={{ id: 'terminal-header' }}>
      <!-- One name policy: the header names the session the same way the
           sidebar row directly beside it does (attention.ts displayName).
           It used to render the tmux name unconditionally, so with the
           default settings the two disagreed about what you were looking
           at. The tmux name stays reachable in the tooltip. -->
      <span class="name" title={$selectedSession.tmux_name}
        >{displayName($selectedSession, $showFriendlyNames)}</span
      >
      <TransferChip session={$selectedSession} />
      <span class="size" data-testid="terminal-size">
        {#if lastCols > 0}{lastCols}×{lastRows}{:else}measuring…{/if}
      </span>
      <span class="counters" data-testid="terminal-counters">{totalBytes}B</span>
      <button
        class="reconnect"
        onclick={() => void openTerm()}
        title="Detach and re-attach"
        data-testid="terminal-reconnect"
      >
        ↻ reconnect
      </button>
    </div>
    <!-- The grid container. tabindex keeps it in the tab order; the focus it
         receives is handed straight to the IME proxy below, which is what
         actually holds the caret. keydown stays here so it catches the
         proxy's keystrokes as they bubble. We render lines as block <div>s
         with monospace spans for each style run. -->
    <!-- role=application, not textbox: a terminal passes keystrokes straight
         through, and unlike textbox it may contain the focusable proxy.
         tabindex=-1 keeps the grid programmatically focusable (the mouse
         controller focuses it) while the proxy stays the single tab stop, so
         Tab and Shift+Tab move past the terminal instead of inside it. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions
         The handlers belong here: the grid is the terminal surface, and the
         rule does not know that `application` delegates keys to the widget. -->
    <div
      class="grid"
      class:kb-focus={focused && keyboardFocus}
      style:--cell-w={cellWidth > 0 ? `${cellWidth}px` : null}
      bind:this={container}
      tabindex="-1"
      role="application"
      aria-label="Terminal"
      onfocus={focusInput}
      onkeydown={onKeydown}
      onwheel={onWheel}
      onmousedown={onMousedown}
      oncontextmenu={onContextMenu}
      data-testid="terminal-host"
    >
      <!-- Hidden probe used once to measure font metrics. We can't rely on
           naive `font-size * 0.6` — system font metrics on macOS drift
           slightly between Menlo and SF Mono. It holds MEASURE_CHARS glyphs,
           not one: the width is divided back down, so per-glyph rounding is
           amortised instead of being multiplied across every run box. -->
      <span class="measure" bind:this={measureCell} aria-hidden="true">{MEASURE_SAMPLE}</span>
      <!-- The real keyboard target. Invisible, one cell wide, parked on the
           cursor so WebKit anchors the IME candidate window and the
           press-and-hold accent popup where the text will land. Its own
           content is never displayed: every handler empties it again. -->
      <textarea
        class="ime-proxy"
        bind:this={imeInput}
        style="left:{caretAt?.left ?? 4}px; top:{caretAt?.top ?? 4}px; height:{caretAt?.h ?? 16}px"
        rows="1"
        autocapitalize="off"
        autocomplete="off"
        {...{ autocorrect: 'off' }}
        spellcheck="false"
        aria-label="Terminal input"
        oncompositionstart={onCompositionStart}
        oncompositionend={onCompositionEnd}
        oninput={onImeInput}
        onfocus={onProxyFocus}
        onblur={onProxyBlur}
        data-ime-proxy="true"
        data-testid="terminal-ime"
      ></textarea>
      {#each visibleRows as row (row.key)}
        <div class="row">
          {#each row.runs as run, i (i)}
            <span
              class:wide={run.wide}
              class:glyph={run.glyph}
              style={runStyle(run)}
              style:--n={run.cells}>{run.text}</span>
          {/each}
        </div>
      {/each}
      {#each selRects as r (r.top + ':' + r.left)}
        <div
          class="selection"
          style="left:{r.left}px; top:{r.top}px; width:{r.width}px; height:{r.height}px"
          aria-hidden="true"
          data-testid="terminal-selection"
        ></div>
      {/each}
      {#if cursor}
        <!-- Keyed on blinkEpoch: each keystroke recreates the element, which
             restarts the blink animation at its visible phase. -->
        {#key blinkEpoch}
          <div
            class="cursor {cursor.shape}"
            class:blink={cursor.blink && focused}
            class:unfocused={!focused}
            style="left:{cursor.left}px; top:{cursor.top}px; width:{cursor.w}px; height:{cursor.h}px"
            aria-hidden="true"
            data-testid="terminal-cursor"
          ></div>
        {/key}
      {/if}
      {#if dragOver || uploading}
        <div class="drop-overlay" data-testid="terminal-drop-overlay">
          {uploading ? 'Uploading…' : `Drop files to upload to ${currentHost ?? 'host'}`}
        </div>
      {/if}
    </div>
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
    {#if openError}
      {#if selectedIsAgentHost && $selectedSession}
        <!-- The attach was tried and failed. `transport: 'agent'` is not a
             claim that no route exists anywhere — only that the hub has
             none — so name what is actually missing: a route from HERE. -->
        <div class="err" data-testid="terminal-agent-transport">
          {$selectedSession.host_alias} is reached through fleet-agent, not SSH, and this machine
          has no SSH route to it. Give it one — an ssh config entry for {$selectedSession.host_alias},
          through a host that can reach it — or move the session somewhere you can attach.
          ({openError})
        </div>
      {:else}
        <div class="err">{openError}</div>
      {/if}
    {/if}
  </div>
{:else}
  <div class="empty" data-testid="terminal-empty">
    <!-- Stroke-only terminal-window icon: traffic lights + chevron prompt with
         a cursor underscore. currentColor lets it ride the theme's muted fg. -->
    <svg
      class="empty-icon"
      viewBox="0 0 64 64"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <rect x="6" y="10" width="52" height="44" rx="5" />
      <line x1="6" y1="21" x2="58" y2="21" />
      <circle cx="13" cy="15.5" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="17.5" cy="15.5" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="22" cy="15.5" r="1.2" fill="currentColor" stroke="none" />
      <polyline points="16,32 22,38 16,44" />
      <line x1="27" y1="44" x2="42" y2="44" />
    </svg>
    <p class="empty-msg">Select a session to attach a terminal.</p>
  </div>
{/if}

<style>
  .wrap {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    min-height: 0;
  }
  .reconnect-banner {
    position: absolute;
    top: 0.4rem;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(180, 100, 100, 0.18);
    color: rgb(220, 130, 130);
    padding: 0.35rem 0.7rem;
    border: 1px solid rgba(220, 130, 130, 0.3);
    border-radius: 5px;
    font-size: 0.8rem;
    z-index: 5;
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .reconnect-banner button {
    font-size: 0.75rem;
    padding: 0.15rem 0.5rem;
    background: transparent;
    border: 1px solid currentColor;
    color: inherit;
    border-radius: 4px;
    cursor: pointer;
  }
  .header {
    flex: 0 0 auto;
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    font-size: 0.85rem;
  }
  .name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: var(--fg);
    font-weight: 600;
  }
  .size {
    margin-left: auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    padding: 0.1rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .counters {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    padding: 0.1rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .reconnect {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .reconnect:hover { color: var(--fg); border-color: var(--accent); }
  .grid {
    position: relative;
    /* Width of one cell, republished from the measured metrics once the font
       is up (see `measureCellSize`). Every run's box is a multiple of it, so
       the text grid, the cursor and the selection overlay share one unit. */
    --cell-w: 1ch;
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    user-select: none;
    -webkit-user-select: none;
    background: #0a0a0a;
    color: #e8e8e8;
    overflow: hidden;
    padding: 4px;
    box-sizing: border-box;
    font-family: Menlo, ui-monospace, SFMono-Regular, monospace;
    font-size: 13px;
    line-height: 16px;
    /* Show focus ring subtly so the user knows where keyboard input lands. */
    outline: none;
  }
  /* Keyboard focus only: the proxy is a text control, and a UA matches
     :focus-visible on those even for a plain mouse click. */
  .grid.kb-focus {
    box-shadow: inset 0 0 0 1px var(--accent, #4f8fff);
  }
  /* Invisible, but NOT display:none / visibility:hidden and not off-screen —
     WebKit only opens an input-method session on an element it considers
     rendered, and positions the candidate window from its caret rect. Inherits
     the grid font so that rect lines up with the cell it sits on. */
  .ime-proxy {
    position: absolute;
    z-index: 2;
    width: 1px;
    min-width: 0;
    padding: 0;
    margin: 0;
    border: 0;
    outline: none;
    resize: none;
    overflow: hidden;
    background: transparent;
    color: transparent;
    caret-color: transparent;
    opacity: 0;
    /* Clicks belong to the grid underneath — this only ever takes focus. */
    pointer-events: none;
    font: inherit;
    line-height: inherit;
    white-space: pre;
  }
  .row {
    white-space: pre;
    /* Use exact cell height so the row count math stays consistent with
       what we measure from `.measure`. */
    height: 16px;
    line-height: 16px;
  }
  .row span {
    /* span color comes from inline style applied per run. Each run is pinned
       to exactly the cells it covers (`--n`), in the same unit the cursor,
       the selection overlay and mouse→cell mapping use — so a run whose
       glyphs are drawn a fraction wider can't push the rest of the row off
       the column grid. */
    display: inline-block;
    width: calc(var(--cell-w) * var(--n, 1));
    height: 16px;
    vertical-align: top;
  }
  /* A single glyph that may come from a fallback font: a wide (2-column)
     emoji or CJK char, or a narrow one outside the grid font's coverage
     (⏺ ⎿ ✻, DEC scan lines, a cell carrying a combining mark). The fallback
     advance is not a whole number of cells, so centre the glyph in its
     pinned box and clip whatever sticks out. */
  .row span.wide,
  .row span.glyph {
    overflow: hidden;
    text-align: center;
  }
  .selection {
    position: absolute;
    background: rgba(120, 170, 255, 0.35);
    pointer-events: none;
    z-index: 1;
  }
  /* Cursor overlay. Translucent so the glyph under it stays readable.
     Position/size are set inline from the measured cell metrics; hidden
     automatically when the app sends ?25l. Shape classes follow DECSCUSR:
     block (default), underline, bar. `blink` is applied only while the grid
     has focus and the app asked for a blinking style; `unfocused` swaps the
     fill for a hollow outline, the standard "input is elsewhere" cue. */
  .cursor {
    position: absolute;
    background: #e8e8e8;
    opacity: 0.55;
    pointer-events: none;
    z-index: 1;
    box-sizing: border-box;
  }
  .cursor.underline {
    background: none;
    border-bottom: 2px solid #e8e8e8;
    opacity: 0.9;
  }
  .cursor.bar {
    background: none;
    border-left: 2px solid #e8e8e8;
    opacity: 0.9;
  }
  .cursor.unfocused {
    background: none;
    border: 1px solid #e8e8e8;
    opacity: 0.6;
  }
  .cursor.blink {
    animation: cf-cursor-blink 1.1s steps(1, end) infinite;
  }
  @keyframes cf-cursor-blink {
    50% { opacity: 0; }
  }
  .drop-overlay {
    position: absolute;
    inset: 0;
    z-index: 4;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(20, 30, 50, 0.55);
    border: 2px dashed var(--accent, #4f8fff);
    color: #e8e8e8;
    font-size: 0.95rem;
    pointer-events: none;
  }
  .measure {
    position: absolute;
    visibility: hidden;
    pointer-events: none;
    font-family: inherit;
    font-size: inherit;
    line-height: inherit;
    /* Place outside flow so it doesn't push the grid around. */
    top: -1000px;
    left: -1000px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 1.1rem;
    height: 100%;
    width: 100%;
    color: var(--fg-muted);
    user-select: none;
  }
  .empty-icon {
    width: 84px;
    height: 84px;
    opacity: 0.55;
  }
  .empty-msg {
    margin: 0;
    font-size: 0.95rem;
    letter-spacing: 0.01em;
  }
  .err {
    flex: 0 0 auto;
    color: #e64a4a;
    font-size: 0.8rem;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid #e64a4a;
  }
  .ctx-backdrop {
    position: fixed;
    inset: 0;
    /* Below the app's modals (z-index 20); above terminal content. */
    z-index: 18;
  }
  .ctx-menu {
    position: fixed;
    z-index: 19;
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
</style>
