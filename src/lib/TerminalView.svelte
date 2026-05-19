<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { Channel, invoke } from '@tauri-apps/api/core';
  import { selectedSession } from './selection';
  import '@xterm/xterm/css/xterm.css';

  let container: HTMLDivElement | undefined = $state(undefined);
  let term: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let currentSession: string | null = $state(null);
  let openError: string | null = $state(null);

  // Drive PTY lifecycle from the selected session in the store. Effects in
  // Svelte 5 re-run whenever any rune they read changes.
  $effect(() => {
    const sess = $selectedSession;
    if (!sess) {
      closeTerm();
      return;
    }
    if (sess.tmux_name === currentSession) return;
    void openTerm(sess.tmux_name);
  });

  // The container ref isn't ready until after the {#if} block mounts the div.
  // Watch for its arrival and re-trigger open if a session is already selected.
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
      theme: readThemeFromCss(),
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    try {
      fitAddon.fit();
    } catch {
      /* fit can throw on zero-size containers; ignore, ResizeObserver will retry */
    }

    const onData = new Channel<string>();
    onData.onmessage = (chunk: string) => {
      term?.write(chunk);
    };

    try {
      await invoke('pty_open', {
        args: {
          sessionName,
          cols: term.cols,
          rows: term.rows,
        },
        onData,
      });
      currentSession = sessionName;
    } catch (e) {
      openError = describeError(e);
      term?.writeln(`\r\n\x1b[31mPTY open failed: ${openError}\x1b[0m\r\n`);
      return;
    }

    term.onData((data) => {
      void invoke('pty_write', { args: { data } }).catch(() => {});
    });

    resizeObserver = new ResizeObserver(() => {
      if (!term || !fitAddon) return;
      try {
        fitAddon.fit();
      } catch {
        return;
      }
      void invoke('pty_resize', {
        args: { cols: term.cols, rows: term.rows },
      }).catch(() => {});
    });
    resizeObserver.observe(container);
  }

  async function closeTerm() {
    resizeObserver?.disconnect();
    resizeObserver = null;
    term?.dispose();
    term = null;
    fitAddon = null;
    if (currentSession) {
      try {
        await invoke('pty_close');
      } catch {
        /* nothing to undo */
      }
      currentSession = null;
    }
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
  }
  .header {
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
  .xterm-host {
    flex: 1;
    min-height: 0;
    background: var(--bg);
    padding: 0.25rem;
    overflow: hidden;
  }
  .empty { color: var(--fg-muted); font-size: 0.9rem; padding: 0.75rem; }
  .err {
    color: #e64a4a;
    font-size: 0.8rem;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid #e64a4a;
  }
</style>
