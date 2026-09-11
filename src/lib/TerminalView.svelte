<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { selectedSession } from './selection';
  import { Screen, rowToRuns, colorToCss, type Run } from './ansi';
  import { pointInRect } from './geometry';
  import { selectionRects, type CellPos } from './terminal_selection';
  import { nativeWriteText } from './clipboard_native';
  import { hintAnchor } from './hints';
  import { toIpcError } from './result';
  import { push, pushError } from './toasts';
  import { repairSession } from './sessions';
  import { keyToBytes, detectMac } from './terminal_keys';
  import { createDrainLoop } from './terminal_drain';
  import { createTerminalClipboard, pathsToPasteText } from './terminal_clipboard';
  import { createMouseController } from './terminal_mouse';

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
  //     Ctrl chords, Alt/Option as an ESC prefix).
  // For our use case (tmux + claude TUI legibly visible in-app) these
  // limits are acceptable.
  // ─────────────────────────────────────────────────────────────────────

  let container: HTMLDivElement | undefined = $state(undefined);
  let measureCell: HTMLSpanElement | undefined = $state(undefined);
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
    return !!sess && sess.tmux_name === currentSession && sess.host_alias === currentHost;
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
  /** Selection endpoints in 0-based grid cells; null when nothing selected.
   *  Held in component state so the drain re-render can't wipe it (unlike the
   *  old window.getSelection() path). */
  let selAnchor: CellPos | null = $state(null);
  let selFocus: CellPos | null = $state(null);
  let openError: string | null = $state(null);
  /** Context-menu position (client px) or null when hidden. */
  let ctxMenu: { x: number; y: number } | null = $state(null);
  let ptyOpen = false;
  let lastCols = $state(0);
  let lastRows = $state(0);
  let totalBytes = $state(0);
  let drainTicks = $state(0);
  /** Measured advance width of a single monospace cell, in px. We compute
   *  this once after mount from a sample <span>. Without a sane fallback
   *  the geometry calc would yield NaN and the view would never size. */
  let cellWidth = 0;
  let cellHeight = 0;
  let disconnected = $state(false);
  /** Self-healing: when the PTY dies (EOF / reader error — e.g. the ssh attach
   *  to a remote host dropped or its ControlMaster wedged), auto-reattach a
   *  bounded number of times with backoff before falling back to the manual
   *  reconnect banner. The budget resets on a fresh selection, a manual
   *  reconnect, or sustained healthy output, so the cap only bites on a
   *  genuinely persistent failure rather than a one-off blip. */
  let autoReconnecting = $state(false);
  let reconnectAttempts = 0;
  let autoReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  const MAX_AUTO_RECONNECT = 3;
  const AUTO_RECONNECT_BASE_MS = 600;

  const drain = createDrainLoop({
    drainOnce,
    attached: () => !!screen && ptyOpen,
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
  });
  const { onWheel, onMousedown } = mouse;

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
    uploading = true;
    try {
      const remote = await invoke<string[]>('upload_to_session', {
        args: { host_alias: currentHost, session_name: currentSession, local_paths: paths },
      });
      if (remote.length > 0) sendPaste(pathsToPasteText(remote));
    } catch (e) {
      openError = `Upload failed: ${describeError(e)}`;
    } finally {
      uploading = false;
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

  async function openTerm(isAutoReconnect = false) {
    if (opening) return;
    const sess = $selectedSession;
    if (!sess) return;
    if (!container) return;
    opening = true;
    // A fresh open (new selection, manual reconnect, detach/reattach button)
    // starts with a clean self-heal budget; an auto-reconnect must preserve the
    // running attempt count so the cap can actually be reached.
    if (!isAutoReconnect) reconnectAttempts = 0;
    await closeTerm();
    openError = null;
    disconnected = false;
    await tick();

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
        if (!r.ok) openError = `Clipboard write failed: ${r.error.message}`;
      });
    };
    renderVersion++;

    // A pane drag fires ResizeObserver every frame; resizing the screen
    // buffer (a full re-mark of every row) and sending pty_resize (a
    // SIGWINCH + tmux redraw over SSH) on each one floods the PTY. Debounce
    // on the trailing edge: only the settled size is applied, and it always
    // is — the last frame of a drag is never dropped.
    resizeObserver = new ResizeObserver(() => {
      if (resizeTimer !== null) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(applyResize, RESIZE_DEBOUNCE_MS);
    });
    resizeObserver.observe(container);

    // Automatic workspace check before attach. It only CREATES what is
    // confirmed missing: re-adds a deleted, unregistered worktree from its
    // existing branch, and starts a tmux session that is confirmed dead. It
    // never respawns a live pane (that would kill a running Claude just
    // because it was selected), never unregisters, adopts or rebranches —
    // those are Repair workspace only; we say so instead. A healthy session
    // costs one probe; orphans and background rows have nothing to check. An
    // offline host is left to the attach error.
    if (sess.project_id != null && sess.kind !== 'bg') {
      const rep = await repairSession(sess.id);
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
      currentSession = sess.tmux_name;
      currentHost = sess.host_alias;
      ptyOpen = true;
    } catch (e) {
      openError = `PTY error: ${describeError(e)}`;
      opening = false;
      return;
    }

    // Start the adaptive drain loop. 30 ms (~33 Hz) is the floor when output
    // is flowing; it backs off to DRAIN_MAX_MS when the terminal is idle.
    drain.start();

    // Hint tmux to redraw at our exact size by re-sending the dimensions
    // once after attach. Defends against race where pty_open runs before
    // the slave-side process has set up SIGWINCH handling.
    setTimeout(() => {
      if (!ptyOpen) return;
      void invoke('pty_resize', { args: { cols: lastCols, rows: lastRows } }).catch(() => {});
    }, 150);
    opening = false;
  }

  const RESIZE_DEBOUNCE_MS = 50;
  let resizeTimer: ReturnType<typeof setTimeout> | null = null;

  function applyResize() {
    resizeTimer = null;
    if (!screen) return;
    const next = computeDimensions();
    if (next.cols === lastCols && next.rows === lastRows) return;
    lastCols = next.cols;
    lastRows = next.rows;
    screen.resize(next.rows, next.cols);
    renderVersion++;
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
    cellWidth = rect.width > 0 ? rect.width : 7.8;
    cellHeight = rect.height > 0 ? rect.height : 16;
  }

  function computeDimensions(): { cols: number; rows: number } {
    if (!container) return { cols: 80, rows: 24 };
    const cw = cellWidth > 0 ? cellWidth : 7.8;
    const ch = cellHeight > 0 ? cellHeight : 16;
    // Subtract our own 4px padding (see CSS) from both sides.
    const w = Math.max(1, container.clientWidth - 8);
    const h = Math.max(1, container.clientHeight - 8);
    return {
      cols: Math.max(10, Math.floor(w / cw)),
      rows: Math.max(2, Math.floor(h / ch)),
    };
  }

  /** Drain the PTY buffer once. Returns true if any bytes were consumed. */
  async function drainOnce(): Promise<boolean> {
    if (!screen || !ptyOpen) return false;
    // Capture the screen we're draining into. If the session is switched
    // (openTerm builds a new Screen) while this pty_drain is in flight, the
    // resolved bytes belong to the old PTY — discard them rather than write
    // stale output into the new screen.
    const drainingInto = screen;
    let result: { data: string; bytes: number };
    try {
      result = await invoke<{ data: string; bytes: number }>('pty_drain');
    } catch {
      return false;
    }
    if (screen !== drainingInto) return false;
    drainTicks += 1;
    if (result.bytes === 0) return false;
    totalBytes += result.bytes;
    screen.write(result.data);
    renderVersion++;
    // Answer any terminal queries (DSR cursor position, DA) the output
    // carried — the parser has no back-channel, so we forward its replies.
    const reply = screen.takeReplies();
    if (reply !== '') writePty(reply);
    // Markers injected by the Rust reader thread when the PTY closes (e.g. the
    // SSH child to a remote host died — now within ~10s thanks to the
    // ServerAlive keepalive in pty.rs, instead of hanging silently forever).
    // Try to self-heal by auto-reattaching; fall back to the manual banner
    // only after the retry budget is exhausted.
    if (result.data.includes('[cf] PTY EOF') || result.data.includes('[cf] reader error')) {
      scheduleAutoReconnect();
    } else if (reconnectAttempts > 0 && !result.data.includes('[cf] attached')) {
      // Real session output after a reconnect (not our own status banner) ⇒
      // the connection is healthy again; restore the self-heal budget.
      reconnectAttempts = 0;
    }
    return true;
  }

  /** Self-healing reattach. Called when the reader thread reports the PTY
   *  died. Schedules a bounded, backed-off reattach for the still-selected
   *  session; once the budget is spent, surfaces the manual banner instead. */
  function scheduleAutoReconnect() {
    if (autoReconnectTimer !== null) return; // one already pending
    if (reconnectAttempts >= MAX_AUTO_RECONNECT) {
      autoReconnecting = false;
      disconnected = true; // give up → manual one-click recovery
      return;
    }
    reconnectAttempts += 1;
    autoReconnecting = true;
    const sessionAtSchedule = currentSession;
    const hostAtSchedule = currentHost;
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
    // Cancel any pending self-heal first — a scheduled auto-reconnect for a
    // now-stale session must never fire after a detach or session switch.
    // (Done before the no-op guard below so a lingering timer is always
    // cleared, and kept conditional so we don't write $state needlessly.)
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

    drain.stop();
    resizeObserver?.disconnect();
    resizeObserver = null;
    if (resizeTimer !== null) {
      clearTimeout(resizeTimer);
      resizeTimer = null;
    }
    screen = null;
    lastCols = 0;
    lastRows = 0;
    totalBytes = 0;
    drainTicks = 0;
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
    // While an IME / dead-key composition is in progress the keydowns are
    // part of composing — the finished text arrives via compositionend.
    if (e.isComposing) return;
    if (e.key === 'Escape' && ctxMenu) { ctxMenu = null; return; }
    const k = e.key.toLowerCase();
    const cmdChord = e.metaKey && !e.altKey && !e.ctrlKey;
    const ctrlShiftChord = !isMac && e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
    // Paste from the native clipboard (bracketed-paste framing in sendPaste).
    // Plain Ctrl+V is intentionally NOT intercepted so ^V reaches the app.
    if ((cmdChord || ctrlShiftChord) && k === 'v') {
      e.preventDefault();
      void pasteFromClipboard();
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
    // Cmd+A → select the whole viewport.
    if (cmdChord && k === 'a') {
      e.preventDefault();
      selAnchor = { row: 0, col: 0 };
      selFocus = { row: lastRows - 1, col: lastCols - 1 };
      return;
    }
    const bytes = keyToBytes(e, { appCursor: screen?.appCursorKeys ?? false, isMac });
    if (bytes === null) return;
    e.preventDefault();
    writePty(bytes);
    // The keystroke will produce output (echo / TUI redraw); pull the drain
    // loop back to full rate so it doesn't sit on a backed-off delay.
    bumpDrain();
  }

  /** Forward IME / dead-key composed text (e.g. Slovak `á`, CJK input) — it
   *  never reaches `onKeydown` as a single printable char. */
  function onCompositionEnd(e: CompositionEvent) {
    if (!ptyOpen || !e.data) return;
    writePty(e.data);
    bumpDrain();
  }

  function describeError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      return String((e as { message: unknown }).message);
    }
    return String(e);
  }

  onDestroy(() => {
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
  // The key encodes the row index followed by every run's style + text. When
  // a row's content changes, its key changes, so Svelte destroys and
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
      // Row index + each run's style/text, joined with control bytes
      // 0x01..0x04. Cells only ever hold printable chars (code >= 0x20), so
      // those bytes never occur in run.text and the fields can't collide.
      let key = String(r);
      for (const run of runs) {
        key += `\u0001${run.fg}\u0002${run.bg}\u0003${run.attrs}\u0004${run.text}`;
      }
      const entry = { ver, key, runs };
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
  const cursor = $derived.by<{ left: number; top: number; w: number; h: number } | null>(() => {
    void renderVersion;
    if (!screen || !screen.cursorVisible) return null;
    if (cellWidth <= 0 || cellHeight <= 0) return null;
    return {
      left: 4 + screen.cursorCol * cellWidth,
      top: 4 + screen.cursorRow * cellHeight,
      w: cellWidth,
      h: cellHeight,
    };
  });

  // Selection highlight rects. Touch renderVersion so it tracks resizes/redraws.
  const selRects = $derived.by(() => {
    void renderVersion;
    if (!selAnchor || !selFocus || cellWidth <= 0 || cellHeight <= 0) return [];
    return selectionRects(selAnchor, selFocus, lastCols, cellWidth, cellHeight, 4);
  });

  function runStyle(run: Run): string {
    const cacheKey = `${run.fg}|${run.bg}|${run.attrs}`;
    const hit = styleCache.get(cacheKey);
    if (hit !== undefined) return hit;
    const parts: string[] = [];
    let fg = colorToCss(run.fg);
    let bg = colorToCss(run.bg);
    // Reverse video (SGR 7 → ATTR_REVERSE): swap fg/bg, substituting the grid
    // defaults for cells that use the default color. This is how claude/tmux
    // draw the input CARET (a reverse-video block) and selections — without it
    // they render as plain text and are invisible.
    if (run.attrs & 16) {
      const f = fg ?? '#e8e8e8'; // grid default text color (.grid color)
      const b = bg ?? '#0a0a0a'; // grid default background (.grid background)
      fg = b;
      bg = f;
    }
    if (fg) parts.push(`color:${fg}`);
    if (bg) parts.push(`background:${bg}`);
    if (run.attrs & 1) parts.push('font-weight:600'); // ATTR_BOLD
    if (run.attrs & 2) parts.push('opacity:0.75'); // ATTR_DIM
    if (run.attrs & 4) parts.push('font-style:italic'); // ATTR_ITALIC
    if (run.attrs & 8) parts.push('text-decoration:underline'); // ATTR_UNDERLINE
    const style = parts.join(';');
    styleCache.set(cacheKey, style);
    return style;
  }
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
      <span class="name">{$selectedSession.tmux_name}</span>
      <span class="host">on {$selectedSession.host_alias}</span>
      <span class="size" data-testid="terminal-size">
        {#if lastCols > 0}{lastCols}×{lastRows}{:else}measuring…{/if}
      </span>
      <span class="counters" data-testid="terminal-counters">
        ticks: {drainTicks} · {totalBytes}B
      </span>
      <button
        class="reconnect"
        onclick={() => void openTerm()}
        title="Detach and re-attach"
        data-testid="terminal-reconnect"
      >
        ↻ reconnect
      </button>
    </div>
    <!-- The grid container. tabindex makes it focusable so keyboard
         input lands here. We render lines as block <div>s with monospace
         spans for each style run. -->
    <div
      class="grid"
      bind:this={container}
      tabindex="0"
      role="textbox"
      aria-label="Terminal"
      aria-multiline="true"
      onkeydown={onKeydown}
      oncompositionend={onCompositionEnd}
      onwheel={onWheel}
      onmousedown={onMousedown}
      oncontextmenu={onContextMenu}
      data-testid="terminal-host"
    >
      <!-- Hidden 1ch×1lh probe used once to measure font metrics. We can't
           rely on naive `font-size * 0.6` — system font metrics on macOS
           drift slightly between Menlo and SF Mono. -->
      <span class="measure" bind:this={measureCell} aria-hidden="true">M</span>
      {#each visibleRows as row (row.key)}
        <div class="row">
          {#each row.runs as run, i (i)}
            <span style={runStyle(run)}>{run.text}</span>
          {/each}
        </div>
      {/each}
      {#each selRects as r (r.top + ':' + r.left)}
        <div
          class="selection"
          style="left:{r.left}px; top:{r.top}px; width:{r.width}px; height:{r.height}px"
          aria-hidden="true"
        ></div>
      {/each}
      {#if cursor}
        <div
          class="cursor"
          style="left:{cursor.left}px; top:{cursor.top}px; width:{cursor.w}px; height:{cursor.h}px"
          aria-hidden="true"
          data-testid="terminal-cursor"
        ></div>
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
      <div class="err">{openError}</div>
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
  .host { color: var(--fg-muted); font-size: 0.75rem; }
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
  .grid:focus-visible {
    box-shadow: inset 0 0 0 1px var(--accent, #4f8fff);
  }
  .row {
    white-space: pre;
    /* Use exact cell height so the row count math stays consistent with
       what we measure from `.measure`. */
    height: 16px;
    line-height: 16px;
  }
  .row span {
    /* span color comes from inline style applied per run. */
    display: inline;
  }
  .selection {
    position: absolute;
    background: rgba(120, 170, 255, 0.35);
    pointer-events: none;
    z-index: 1;
  }
  /* Block cursor overlay. Translucent so the glyph under it stays readable;
     blinks like a standard terminal cursor. Position/size are set inline from
     the measured cell metrics. Hidden automatically when the app sends ?25l. */
  .cursor {
    position: absolute;
    background: #e8e8e8;
    opacity: 0.55;
    pointer-events: none;
    z-index: 1;
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
