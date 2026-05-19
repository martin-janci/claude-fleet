<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { invoke } from '@tauri-apps/api/core';
  import { selectedSession } from './selection';
  import { setPtyWriteSink, ptyDebug } from './pty-stream';
  import '@xterm/xterm/css/xterm.css';

  let container: HTMLDivElement | undefined = $state(undefined);
  let term: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let resizeObserver: ResizeObserver | null = null;
  // Tracks the session that is currently OPEN on the PTY backend. Used to
  // detect when we need to swap, and to gate input-forwarding wiring until
  // the PTY is actually live.
  let currentSession: string | null = $state(null);
  let openError: string | null = $state(null);
  // Set inside the resize observer once the container has a real size and
  // the PTY has been successfully opened.
  let ptyOpen = false;
  // Latest layout (used by pty_resize) — held outside reactive state so we
  // don't rerun effects on every resize.
  let lastCols = 0;
  let lastRows = 0;
  // Reactive copies for the status badge in the header.
  let displayCols = $state(0);
  let displayRows = $state(0);
  // Whether we currently own the global PTY write sink. Set on first PTY open,
  // cleared on closeTerm.
  let sinkAttached = false;
  // Diagnostic counters surfaced in the status footer next to the size badge.
  let sinkCalls = $state(0);
  let sinkBytes = $state(0);
  let writeErrors = $state(0);
  let lastWriteError: string | null = $state(null);

  $effect(() => {
    const sess = $selectedSession;
    if (!sess) {
      void closeTerm();
      return;
    }
    if (sess.tmux_name === currentSession) return;
    void openTerm(sess.tmux_name);
  });

  // The container <div> arrives after the {#if $selectedSession} block
  // mounts. When it does, openTerm() may have been short-circuited (no
  // container yet) — retry once container is bound.
  $effect(() => {
    if (container && $selectedSession && currentSession !== $selectedSession.tmux_name) {
      void openTerm($selectedSession.tmux_name);
    }
  });

  async function openTerm(sessionName: string) {
    if (!container) return;
    await closeTerm();
    openError = null;

    term = new Terminal({
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
      fontSize: 13,
      cursorBlink: true,
      allowProposedApi: true,
      scrollback: 5000,
      convertEol: true,
      theme: readThemeFromCss(),
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);

    // Diagnostic: if you ever see a blank pane, this line confirms xterm
    // itself is rendering. Cleared by tmux's first refresh.
    term.writeln(`\x1b[90m[claude-fleet] attaching to ${sessionName}…\x1b[0m`);

    // Drive both the initial open AND every subsequent resize from a single
    // ResizeObserver, so we never hand pty_open a 0x0 measurement.
    resizeObserver = new ResizeObserver((entries) => {
      const t = term;
      const fa = fitAddon;
      if (!t || !fa) return;
      const entry = entries[0];
      if (entry && (entry.contentRect.width < 4 || entry.contentRect.height < 4)) {
        return;
      }
      try {
        fa.fit();
      } catch {
        return;
      }
      if (t.cols < 2 || t.rows < 2) return;
      displayCols = t.cols;
      displayRows = t.rows;
      if (t.cols === lastCols && t.rows === lastRows && ptyOpen) return;
      lastCols = t.cols;
      lastRows = t.rows;
      if (!ptyOpen) {
        void firstOpen(sessionName);
      } else {
        void invoke('pty_resize', {
          args: { cols: t.cols, rows: t.rows },
        }).catch(() => {});
      }
    });
    resizeObserver.observe(container);

    // Fast-path with retries. rAF fires after layout in most cases, but
    // when the flex pane is still measuring (Tauri webview on first paint
    // can be sluggish) the first frame may still report 0×0. Retry up to
    // 8 frames before giving up and letting ResizeObserver take over.
    tryFitAndOpen(sessionName, 0);
  }

  function tryFitAndOpen(sessionName: string, attempt: number) {
    requestAnimationFrame(() => {
      if (!term || !fitAddon || !container) return;
      try {
        fitAddon.fit();
      } catch {
        if (attempt < 8) tryFitAndOpen(sessionName, attempt + 1);
        return;
      }
      if (term.cols < 2 || term.rows < 2) {
        if (attempt < 8) tryFitAndOpen(sessionName, attempt + 1);
        return;
      }
      displayCols = term.cols;
      displayRows = term.rows;
      if (!ptyOpen) {
        lastCols = term.cols;
        lastRows = term.rows;
        void firstOpen(sessionName);
      }
    });
  }

  async function firstOpen(sessionName: string) {
    if (ptyOpen) return;
    if (!term) return;
    ptyOpen = true; // optimistic: prevents races; we revert on failure

    // Install our xterm as the write sink for the GLOBAL pty-data
    // listener (registered at App boot). No async timing race possible —
    // the listener has been alive since startup.
    setPtyWriteSink((chunk) => {
      sinkCalls += 1;
      sinkBytes += chunk.length;
      if (!term) return;
      try {
        term.write(chunk);
      } catch (e) {
        writeErrors += 1;
        lastWriteError = String(e);
        // eslint-disable-next-line no-console
        console.error('xterm write failed:', e, 'chunk len=', chunk.length);
      }
    });
    sinkAttached = true;

    try {
      await invoke('pty_open', {
        args: {
          session_name: sessionName,
          cols: lastCols,
          rows: lastRows,
        },
      });
      currentSession = sessionName;
    } catch (e) {
      openError = describeError(e);
      term?.writeln(`\r\n\x1b[31mPTY open failed: ${openError}\x1b[0m\r\n`);
      ptyOpen = false;
      setPtyWriteSink(null);
      sinkAttached = false;
      return;
    }

    term.onData((data) => {
      void invoke('pty_write', { args: { data } }).catch(() => {});
    });

    // tmux may have decided its window-size based on whatever client
    // attached first, or may not yet have drawn for our just-set
    // geometry. A redundant pty_resize triggers SIGWINCH on the child,
    // which makes tmux force a full redraw at the correct dimensions.
    setTimeout(() => {
      if (!term || !ptyOpen) return;
      void invoke('pty_resize', {
        args: { cols: term.cols, rows: term.rows },
      }).catch(() => {});
    }, 150);
  }

  async function closeTerm() {
    resizeObserver?.disconnect();
    resizeObserver = null;
    if (sinkAttached) {
      setPtyWriteSink(null);
      sinkAttached = false;
    }
    term?.dispose();
    term = null;
    fitAddon = null;
    lastCols = 0;
    lastRows = 0;
    displayCols = 0;
    displayRows = 0;
    sinkCalls = 0;
    sinkBytes = 0;
    writeErrors = 0;
    lastWriteError = null;
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

  function describeError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      return String((e as { message: unknown }).message);
    }
    return String(e);
  }

  function readThemeFromCss(): { background: string; foreground: string } {
    if (typeof window === 'undefined') return { background: '#0f0f0f', foreground: '#ededed' };
    const cs = getComputedStyle(document.documentElement);
    const bg = cs.getPropertyValue('--bg').trim() || '#0f0f0f';
    const fg = cs.getPropertyValue('--fg').trim() || '#ededed';
    return { background: bg, foreground: fg };
  }

  onDestroy(() => {
    void closeTerm();
  });
</script>

{#if $selectedSession}
  <div class="wrap">
    <div class="header" data-testid="terminal-header">
      <span class="name">{$selectedSession.tmux_name}</span>
      <span class="host">on {$selectedSession.host_alias}</span>
      <span class="size" data-testid="terminal-size">
        {#if displayCols > 0}{displayCols}×{displayRows}{:else}measuring…{/if}
      </span>
      <span class="counters" data-testid="terminal-counters">
        sink: {sinkCalls} · {sinkBytes}B{#if writeErrors > 0} · err {writeErrors}{/if}
      </span>
      <button
        class="reconnect"
        onclick={() => {
          // Test: write a known string directly to xterm. If THIS doesn't
          // appear in the pane, xterm itself is broken; if it does, the
          // chain from sink → term.write is OK and the bug is elsewhere.
          term?.write('\r\n\x1b[93m[test write] hello from JS @ ' + new Date().toISOString() + '\x1b[0m\r\n');
        }}
        title="Write a synthetic line directly to xterm"
        data-testid="terminal-test-write"
      >
        ✎ test
      </button>
      <button
        class="reconnect"
        onclick={() => $selectedSession && void openTerm($selectedSession.tmux_name)}
        title="Detach and re-attach"
        data-testid="terminal-reconnect"
      >
        ↻ reconnect
      </button>
    </div>
    <div class="xterm-host" bind:this={container} data-testid="terminal-host"></div>
    {#if openError}
      <div class="err">PTY error: {openError}</div>
    {/if}
    {#if $ptyDebug.length > 0}
      <details class="debug">
        <summary>pty-data events ({$ptyDebug.length}) — click to expand</summary>
        <ul>
          {#each $ptyDebug.slice(-10).reverse() as e (e.ts)}
            <li><code>{e.bytes}B · {e.preview}</code></li>
          {/each}
        </ul>
      </details>
    {/if}
  </div>
{:else}
  <p class="empty" data-testid="terminal-empty">Select a session to attach a terminal.</p>
{/if}

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    min-height: 0;
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
  .xterm-host {
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    background: var(--bg);
    overflow: hidden;
    position: relative;
  }
  /* xterm.js inserts its own .xterm root inside .xterm-host. Force it to
     fill the available area so FitAddon measures the right rect. */
  .xterm-host :global(.xterm) {
    height: 100%;
    width: 100%;
    padding: 4px;
    box-sizing: border-box;
  }
  .xterm-host :global(.xterm-viewport) {
    background-color: transparent !important;
  }
  .empty { color: var(--fg-muted); font-size: 0.9rem; padding: 0.75rem; }
  .err {
    flex: 0 0 auto;
    color: #e64a4a;
    font-size: 0.8rem;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid #e64a4a;
  }
  .debug {
    flex: 0 0 auto;
    font-size: 0.7rem;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg-muted);
    max-height: 30%;
    overflow-y: auto;
  }
  .debug summary { cursor: pointer; }
  .debug ul { list-style: none; margin: 0.3rem 0 0 0; padding: 0; }
  .debug li { padding: 0.1rem 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
</style>
