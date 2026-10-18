<script lang="ts">
  // An extra fleet's session, attached (multi-hub Phase 2). The attach is
  // this machine's own `ssh … tmux attach` (`pty_open`, the same in both
  // modes, design Risks), so it works where this machine's ssh knows the
  // fleet's host alias and fails with ssh's own words where it does not.
  //
  // Deliberately NOT TerminalView: that pane is bound to the primary's
  // selection — its workspace check, voice claim, shell tabs and access gate
  // all read `selectedSession` as an id in the primary's space, where this
  // session's id names another one. This is the same renderer (`ansi.ts`,
  // `terminal_keys.ts`, `terminal_drain.ts`) under its own pty id
  // (`fleetPtyId`), with none of that.
  import { onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { Screen, rowToRuns, runStyleCss, type Run } from './ansi';
  import { keyToBytes, detectMac } from './terminal_keys';
  import { createDrainLoop } from './terminal_drain';
  import { fitCells } from './terminal_size';
  import { fleetPtyId } from './fleets';
  import { toIpcError } from './result';
  import type { SessionRow } from './sessions';
  import Button from './kit/Button.svelte';

  let { fleet, session }: { fleet: string; session: SessionRow } = $props();

  interface PtyDrainResult {
    data: string;
    bytes: number;
    eof: boolean;
    overflowed: boolean;
  }

  const CELL_H = 16;
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);

  let grid: HTMLDivElement | undefined = $state(undefined);
  let probe: HTMLSpanElement | undefined = $state(undefined);
  let screen: Screen | null = null;
  let version = $state(0);
  let ptyId: string | null = null;
  let generation: number | null = null;
  let open = $state(false);
  let error = $state<string | null>(null);
  let ended = $state(false);
  let cols = 80;
  let rows = 24;
  let cellW = $state(7.8);
  let ro: ResizeObserver | null = null;

  const view = $derived.by(() => {
    void version;
    const scr = screen;
    if (!scr) return [] as Run[][];
    return scr.cells.map((r) => rowToRuns(r));
  });

  function measure() {
    const w = probe?.getBoundingClientRect().width ?? 0;
    if (w > 0) cellW = w / 20;
  }

  function size(): { cols: number; rows: number } {
    const box = grid?.getBoundingClientRect();
    if (!box || box.width <= 0) return { cols, rows };
    return fitCells(box.width - 8, box.height - 8, cellW, CELL_H);
  }

  function write(data: string) {
    if (!ptyId || !open) return;
    void invoke('pty_write', { args: { id: ptyId, data } }).catch(() => {});
    drain.bumpDrain();
  }

  async function drainOnce(): Promise<boolean> {
    if (!screen || !ptyId || !open) return false;
    const into = screen;
    let r: PtyDrainResult;
    try {
      r = await invoke<PtyDrainResult>('pty_drain', {
        args: generation === null ? { id: ptyId } : { id: ptyId, generation },
      });
    } catch {
      return false;
    }
    if (screen !== into) return false;
    if (r.overflowed) {
      // The stream lost the modes tmux sends once per attach: re-attach.
      void attach();
      return false;
    }
    if (r.bytes > 0) {
      into.write(r.data);
      into.takeRowShifts();
      const reply = into.takeReplies();
      if (reply !== '') write(reply);
      version++;
    }
    if (r.eof) {
      open = false;
      ended = true;
    }
    return r.bytes > 0;
  }

  const drain = createDrainLoop({ drainOnce, attached: () => !!screen && open });

  async function close() {
    const id = ptyId;
    open = false;
    if (id) await invoke('pty_close', { args: { id } }).catch(() => {});
  }

  async function attach() {
    await close();
    measure();
    const dim = size();
    cols = dim.cols;
    rows = dim.rows;
    screen = new Screen(rows, cols);
    version++;
    error = null;
    ended = false;
    ptyId = fleetPtyId(fleet, session);
    try {
      const opened = await invoke<unknown>('pty_open', {
        args: { id: ptyId, session_name: session.tmux_name, host_alias: session.host_alias, cols, rows },
      });
      generation = typeof opened === 'number' ? opened : null;
      open = true;
      drain.start();
      grid?.focus();
    } catch (e) {
      error = toIpcError(e).message;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (!open) return;
    const bytes = keyToBytes(e, { appCursor: screen?.appCursorKeys ?? false, isMac });
    if (bytes === null) return;
    e.preventDefault();
    write(bytes);
  }

  function onPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData('text/plain');
    if (!text || !open) return;
    e.preventDefault();
    write(screen?.bracketedPaste ? `\x1b[200~${text}\x1b[201~` : text);
  }

  onMount(() => {
    void attach();
    if (typeof ResizeObserver !== 'undefined' && grid) {
      ro = new ResizeObserver(() => {
        if (!screen || !ptyId) return;
        const dim = size();
        if (dim.cols === cols && dim.rows === rows) return;
        cols = dim.cols;
        rows = dim.rows;
        screen.resize(rows, cols);
        version++;
        if (open) void invoke('pty_resize', { args: { id: ptyId, cols, rows } }).catch(() => {});
      });
      ro.observe(grid);
    }
  });

  onDestroy(() => {
    ro?.disconnect();
    drain.stop();
    void close();
  });
</script>

<div class="fleet-term" data-testid="fleet-terminal">
  {#if error}
    <p class="note err" data-testid="fleet-terminal-error">
      Couldn't attach from this machine: {error}. The attach uses this machine's ssh with the
      host alias <code>{session.host_alias}</code>, which has to resolve here too.
    </p>
    <Button size="sm" variant="quiet" onclick={() => void attach()}>Try again</Button>
  {:else if ended}
    <p class="note">The terminal closed.</p>
    <Button size="sm" variant="quiet" onclick={() => void attach()}>Reattach</Button>
  {/if}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="grid"
    bind:this={grid}
    tabindex="0"
    role="application"
    aria-label="Terminal for {session.tmux_name}"
    style:--cell-w="{cellW}px"
    onkeydown={onKeydown}
    onpaste={onPaste}
  >
    <span class="probe" bind:this={probe} aria-hidden="true">{'M'.repeat(20)}</span>
    {#each view as runs, r (r)}
      <div class="row">
        {#each runs as run, i (i)}<span style={runStyleCss(run)} style:--n={run.cells}>{run.text}</span>{/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .fleet-term {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    height: 100%;
    min-height: 0;
  }
  .grid {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    padding: 4px;
    background: var(--term-bg);
    color: var(--term-fg);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    line-height: 16px;
    outline: none;
    border-radius: var(--radius-sm);
  }
  .grid:focus-visible {
    box-shadow: inset 0 0 0 var(--ring-w) var(--ring);
  }
  .probe {
    position: absolute;
    visibility: hidden;
    white-space: pre;
  }
  .row {
    white-space: pre;
    height: 16px;
  }
  .row span {
    display: inline-block;
    width: calc(var(--cell-w) * var(--n, 1));
    height: 16px;
    vertical-align: top;
    overflow: hidden;
  }
  .note {
    margin: 0;
    font-size: var(--text-sm);
  }
  .err {
    color: var(--danger);
  }
</style>
