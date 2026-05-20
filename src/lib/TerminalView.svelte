<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Terminal } from 'xterm';
  import { FitAddon } from 'xterm-addon-fit';
  import { invoke } from '@tauri-apps/api/core';
  import { selectedSession } from './selection';
  import 'xterm/css/xterm.css';

  let container: HTMLDivElement | undefined = $state(undefined);
  let term: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let drainTimer: ReturnType<typeof setInterval> | null = null;
  let currentSession: string | null = $state(null);
  let openError: string | null = $state(null);
  let ptyOpen = false;
  let lastCols = 0;
  let lastRows = 0;
  let displayCols = $state(0);
  let displayRows = $state(0);
  let totalBytes = $state(0);
  let drainTicks = $state(0);

  $effect(() => {
    const sess = $selectedSession;
    if (!sess) {
      void closeTerm();
      return;
    }
    if (sess.tmux_name === currentSession) return;
    void openTerm(sess.tmux_name);
  });

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
      fontFamily: 'Menlo, ui-monospace, SFMono-Regular, monospace',
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      // Force high-contrast theme to rule out CSS-var resolution issues —
      // if --bg/--fg happen to resolve to the same value (unlikely but
      // possible in some color-mode races), xterm would render invisible.
      theme: {
        background: '#0a0a0a',
        foreground: '#e8e8e8',
        cursor: '#ffffff',
        cursorAccent: '#0a0a0a',
        selectionBackground: '#3a3a55',
      },
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    // Diagnostic line — proves xterm renders SOMETHING. If you don't see
    // this line, the bug is in xterm DOM mounting / measurement.
    term.writeln('\x1b[93m[xterm boot] terminal opened\x1b[0m');

    resizeObserver = new ResizeObserver((entries) => {
      const t = term;
      const fa = fitAddon;
      if (!t || !fa) return;
      const entry = entries[0];
      if (entry && (entry.contentRect.width < 4 || entry.contentRect.height < 4)) return;
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
        void invoke('pty_resize', { args: { cols: t.cols, rows: t.rows } }).catch(() => {});
      }
    });
    resizeObserver.observe(container);

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
    ptyOpen = true;

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
      return;
    }

    term.onData((data) => {
      void invoke('pty_write', { args: { data } }).catch(() => {});
    });

    // Start the drain loop. 30 ms = ~33 Hz, fast enough for interactive
    // terminals without burning CPU. The kernel issues SIGWINCH at our
    // next resize anyway, so tmux re-renders correctly.
    drainTimer = setInterval(drainOnce, 30);

    setTimeout(() => {
      if (!term || !ptyOpen) return;
      void invoke('pty_resize', { args: { cols: term.cols, rows: term.rows } }).catch(() => {});
    }, 150);
  }

  async function drainOnce() {
    if (!term || !ptyOpen) return;
    let result: { data: string; bytes: number };
    try {
      result = await invoke<{ data: string; bytes: number }>('pty_drain');
    } catch {
      return;
    }
    drainTicks += 1;
    if (result.bytes === 0) return;
    totalBytes += result.bytes;
    try {
      term.write(result.data);
    } catch (e) {
      // eslint-disable-next-line no-console
      console.error('xterm write failed:', e);
    }
  }

  async function closeTerm() {
    if (drainTimer) {
      clearInterval(drainTimer);
      drainTimer = null;
    }
    resizeObserver?.disconnect();
    resizeObserver = null;
    term?.dispose();
    term = null;
    fitAddon = null;
    lastCols = 0;
    lastRows = 0;
    displayCols = 0;
    displayRows = 0;
    totalBytes = 0;
    drainTicks = 0;
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
        ticks: {drainTicks} · {totalBytes}B
      </span>
      <button
        class="reconnect"
        onclick={() => {
          term?.writeln('\r\n\x1b[93m[test write] ' + new Date().toISOString() + '\x1b[0m');
        }}
        title="Synchronous writeln test"
        data-testid="terminal-test"
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
</style>
