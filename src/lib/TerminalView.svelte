<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { selectedSession } from './selection';
  import { Screen, rowToRuns, colorToCss, type Run } from './ansi';

  // ─────────────────────────────────────────────────────────────────────
  // Terminal pane — minimal ANSI renderer.
  //
  // We do NOT use xterm.js. In our Tauri 2.11 + macOS WKWebView setup,
  // xterm's renderer silently fails to repaint after the first write for
  // reasons we couldn't isolate without devtools (see commit history on
  // branch `main` around 2026-05). Instead we maintain a virtual screen
  // buffer (`./ansi.ts`) and render it as styled `<div>` rows, which is
  // dirt-simple DOM that we can prove repaints. Tradeoffs:
  //   - No mouse tracking, no application keypad, no scrollback beyond
  //     what tmux's own scroll buffer can show with C-b [.
  //   - No wide-glyph (CJK / emoji) width fixups.
  //   - Keyboard input is forwarded as raw bytes; we translate the most
  //     common keys (Enter/Backspace/arrows/Ctrl-*) and let everything
  //     else go via printable character.
  // For our use case (tmux + claude TUI legibly visible in-app) these
  // limits are acceptable.
  // ─────────────────────────────────────────────────────────────────────

  let container: HTMLDivElement | undefined = $state(undefined);
  let measureCell: HTMLSpanElement | undefined = $state(undefined);
  let screen: Screen | null = null;
  /** Bumped after every screen.write() so the reactive view recomputes. */
  let renderVersion = $state(0);
  let resizeObserver: ResizeObserver | null = null;
  // Drain loop: a self-rescheduling setTimeout (not setInterval) so a slow
  // pty_drain round-trip can't pile up concurrent calls. The delay backs off
  // adaptively — an idle terminal polls slowly, any output snaps it back to
  // full rate — so an attached-but-quiet session costs almost nothing.
  const DRAIN_MIN_MS = 30;
  const DRAIN_MAX_MS = 250;
  let drainTimer: ReturnType<typeof setTimeout> | null = null;
  let drainDelay = DRAIN_MIN_MS;
  let currentSession: string | null = $state(null);
  let openError: string | null = $state(null);
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

  $effect(() => {
    const sess = $selectedSession;
    if (!sess) {
      void closeTerm();
      return;
    }
    if (sess.tmux_name === currentSession) return;
    void openTerm();
  });

  // When the container element first appears after a selection (Svelte 5
  // mounts the {#if} block lazily) and we're not yet attached, open the
  // PTY. Required because the first effect above can fire before the
  // <div bind:this> has populated `container`.
  $effect(() => {
    if (container && $selectedSession && currentSession !== $selectedSession.tmux_name) {
      void openTerm();
    }
  });

  /** Reentrancy guard. The two $effects above can both call openTerm() for
   *  the same selection in one flush; openTerm is async and its
   *  `currentSession` guard is only set after `await pty_open`, so a second
   *  call would otherwise run a full second open — leaking a ResizeObserver
   *  and a drain timer and double-opening the PTY. */
  let opening = false;

  async function openTerm() {
    if (opening) return;
    const sess = $selectedSession;
    if (!sess) return;
    if (!container) return;
    opening = true;
    await closeTerm();
    openError = null;
    disconnected = false;
    await tick();

    measureCellSize();
    const dim = computeDimensions();
    lastCols = dim.cols;
    lastRows = dim.rows;
    screen = new Screen(dim.rows, dim.cols);
    renderVersion++;

    resizeObserver = new ResizeObserver(() => {
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
    });
    resizeObserver.observe(container);

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
      ptyOpen = true;
    } catch (e) {
      openError = describeError(e);
      opening = false;
      return;
    }

    // Start the adaptive drain loop. 30 ms (~33 Hz) is the floor when output
    // is flowing; it backs off to DRAIN_MAX_MS when the terminal is idle.
    drainDelay = DRAIN_MIN_MS;
    scheduleDrain();

    // Hint tmux to redraw at our exact size by re-sending the dimensions
    // once after attach. Defends against race where pty_open runs before
    // the slave-side process has set up SIGWINCH handling.
    setTimeout(() => {
      if (!ptyOpen) return;
      void invoke('pty_resize', { args: { cols: lastCols, rows: lastRows } }).catch(() => {});
    }, 150);
    opening = false;
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

  function scheduleDrain() {
    drainTimer = setTimeout(runDrain, drainDelay);
  }

  /** One drain tick, then reschedule itself. The delay halves to the floor on
   *  any output and doubles toward DRAIN_MAX_MS when idle. */
  async function runDrain() {
    drainTimer = null;
    const got = await drainOnce();
    drainDelay = got ? DRAIN_MIN_MS : Math.min(DRAIN_MAX_MS, drainDelay * 2);
    // Reschedule only if still attached and no newer loop has taken over
    // (a concurrent openTerm would have set its own drainTimer).
    if (screen && ptyOpen && drainTimer === null) scheduleDrain();
  }

  /** Force the loop back to full rate now — called on keypress so typing
   *  feels responsive even if the terminal had backed off while idle. */
  function bumpDrain() {
    drainDelay = DRAIN_MIN_MS;
    if (drainTimer !== null) {
      clearTimeout(drainTimer);
      drainTimer = null;
      scheduleDrain();
    }
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
    // Markers injected by the Rust reader thread when the PTY closes (e.g.
    // the SSH child to a remote host died). Surface a reconnect banner so
    // the user has a one-click recovery path.
    if (result.data.includes('[cf] PTY EOF') || result.data.includes('[cf] reader error')) {
      disconnected = true;
    }
    return true;
  }

  async function reconnect() {
    disconnected = false;
    await closeTerm();
    await openTerm();
  }

  async function closeTerm() {
    // No-op when there's nothing to clean up. Without this guard the
    // mount-time effect fires closeTerm() against a fresh component,
    // unconditionally writes state ($state assignments), and Svelte 5's
    // reactivity scheduler treats the cascade as an effect-update loop.
    const hadAnything = screen !== null || ptyOpen || drainTimer !== null || resizeObserver !== null;
    if (!hadAnything) return;

    if (drainTimer) {
      clearTimeout(drainTimer);
      drainTimer = null;
    }
    drainDelay = DRAIN_MIN_MS;
    resizeObserver?.disconnect();
    resizeObserver = null;
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
  }

  /** Translate a KeyboardEvent into the byte sequence a real terminal would
   *  send. Returns null for keys we choose not to forward (e.g. F-keys).
   *  This is intentionally minimal — most apps only need printable chars,
   *  Enter, Backspace, Tab, arrows, and Ctrl-letter chords. */
  function keyToBytes(e: KeyboardEvent): string | null {
    if (e.key === 'Enter') return '\r';
    if (e.key === 'Backspace') return '\x7f';
    if (e.key === 'Tab') return '\t';
    if (e.key === 'Escape') return '\x1b';
    if (e.key === 'ArrowUp') return '\x1b[A';
    if (e.key === 'ArrowDown') return '\x1b[B';
    if (e.key === 'ArrowRight') return '\x1b[C';
    if (e.key === 'ArrowLeft') return '\x1b[D';
    if (e.key === 'Home') return '\x1b[H';
    if (e.key === 'End') return '\x1b[F';
    if (e.key === 'PageUp') return '\x1b[5~';
    if (e.key === 'PageDown') return '\x1b[6~';
    // Ctrl + letter / common chord: send the C0 control byte.
    if (e.ctrlKey && e.key.length === 1) {
      const k = e.key.toLowerCase();
      if (k >= 'a' && k <= 'z') {
        return String.fromCharCode(k.charCodeAt(0) - 96);
      }
      if (k === ' ') return '\x00';
      if (k === '[') return '\x1b';
      if (k === '\\') return '\x1c';
      if (k === ']') return '\x1d';
    }
    // A printable single character: forward as-is.
    if (e.key.length === 1 && !e.metaKey) return e.key;
    return null;
  }

  function onKeydown(e: KeyboardEvent) {
    if (!ptyOpen) return;
    const bytes = keyToBytes(e);
    if (bytes === null) return;
    e.preventDefault();
    void invoke('pty_write', { args: { data: bytes } }).catch(() => {});
    // The keystroke will produce output (echo / TUI redraw); pull the drain
    // loop back to full rate so it doesn't sit on a backed-off delay.
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

  function runStyle(run: Run): string {
    const cacheKey = `${run.fg}|${run.bg}|${run.attrs}`;
    const hit = styleCache.get(cacheKey);
    if (hit !== undefined) return hit;
    const parts: string[] = [];
    const fg = colorToCss(run.fg);
    const bg = colorToCss(run.bg);
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
    {#if disconnected}
      <div class="reconnect-banner" data-testid="terminal-reconnect-banner">
        Connection lost.
        <button onclick={reconnect}>Reconnect</button>
      </div>
    {/if}
    <div class="header" data-testid="terminal-header">
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
      onkeydown={onKeydown}
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
    </div>
    {#if openError}
      <div class="err">PTY error: {openError}</div>
    {/if}
  </div>
{:else}
  <p class="empty" data-testid="terminal-empty">Select a session to attach a terminal.</p>
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
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
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
  .empty { color: var(--fg-muted); font-size: 0.9rem; padding: 0.75rem; }
  .err {
    flex: 0 0 auto;
    color: #e64a4a;
    font-size: 0.8rem;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid #e64a4a;
  }
</style>
