<script lang="ts">
  import { tablistKeys } from './tablist_keys';
  /**
   * The session's terminal tabs (redesign step 5.3, Terminals board): the
   * agent's own screen, then shell terminals 1..N, with + New, Split and
   * Clear. Pure presentation: `TerminalView` owns the list and the actions.
   */
  import {
    MAX_SHELL_TERMINALS,
    TERMINAL_STARTS,
    terminalStartLabel,
    type ShellActivity,
    type TerminalStart,
  } from './terminals';
  import { shortcutLabel } from './shortcuts';
  import { detectMac } from './terminal_keys';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { tick } from 'svelte';

  let {
    agentLabel = 'Claude Code',
    shells,
    active,
    split,
    busy = false,
    onselect,
    onnew,
    onclose,
    onsplit,
    onclear,
    onpopout = undefined,
    host = null,
    opensOn = 'worktree',
    onopenson = undefined,
    activity = {},
  }: {
    /** The agent tab's name, as the session bar names it. */
    agentLabel?: string;
    shells: readonly number[];
    /** The picked tab; `null` is the agent. */
    active: number | null;
    split: boolean;
    busy?: boolean;
    onselect: (n: number | null) => void;
    onnew: () => void;
    onclose: (n: number) => void;
    onsplit: () => void;
    onclear: () => void;
    /** Pop the picked tab out into its own window (step 5.4). */
    onpopout?: () => void;
    /** The session's host, for the "New terminal opens on" picker; null
     *  hides the picker. */
    host?: string | null;
    /** Where + New starts the next terminal (Terminals board). */
    opensOn?: TerminalStart;
    onopenson?: (at: TerminalStart) => void;
    /** What runs in each shell, by number (Terminals board, "pnpm dev ·
     *  running"); a shell missing here says nothing. */
    activity?: Readonly<Record<number, ShellActivity>>;
  } = $props();

  function shellTitle(n: number): string {
    const a = activity[n];
    const what = !a ? '' : a.state === 'running' ? ` · ${a.command} running` : ` · ${a.command}, at its prompt`;
    return `Shell ${n}${what} · next tab ${nextChord}`;
  }

  // The shell's actions menu (Terminals board, "Terminal actions"): the
  // ⋯ button or a right-click on a shell tab. Kill terminal… asks first.
  let menuFor = $state<number | null>(null);
  let killAsk = $state<number | null>(null);
  let menuEl = $state<HTMLElement | null>(null);

  async function openMenu(n: number) {
    menuFor = n;
    await tick();
    menuEl?.querySelector<HTMLElement>('[role=menuitem]')?.focus();
  }
  function closeMenu() {
    menuFor = null;
  }
  function menuKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = Array.from(menuEl?.querySelectorAll<HTMLElement>('[role=menuitem]') ?? []);
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length].focus();
  }
  function pick(fn: () => void) {
    closeMenu();
    fn();
  }

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const newChord = shortcutLabel('new-terminal', isMac);
  const nextChord = shortcutLabel('next-terminal', isMac);
  const full = $derived(shells.length >= MAX_SHELL_TERMINALS);
</script>

<div class="strip" role="tablist" aria-label="Terminals" data-testid="terminal-strip" use:tablistKeys>
  <button
    type="button"
    role="tab"
    class="tab"
    class:on={active === null}
    aria-selected={active === null}
    title="The agent's screen · next tab {nextChord}"
    onclick={() => onselect(null)}
    data-testid="terminal-tab-agent">{agentLabel}</button
  >
  {#each shells as n (n)}
    <span class="tab-wrap" class:on={active === n}>
      <button
        type="button"
        role="tab"
        class="tab"
        class:on={active === n}
        aria-selected={active === n}
        title={shellTitle(n)}
        onclick={() => onselect(n)}
        oncontextmenu={(e) => {
          e.preventDefault();
          onselect(n);
          void openMenu(n);
        }}
        data-testid="terminal-tab-{n}"
        >Shell {n}{#if activity[n]?.state === 'running'}<span class="run" data-testid="terminal-running-{n}"
            ><span class="run-dot" aria-hidden="true"></span>{activity[n]?.command}<span class="sr-only"> running</span></span
          >{/if}</button
      >
      <button
        type="button"
        class="close"
        aria-label="Close Shell {n}"
        title="Close Shell {n}. The session keeps running."
        disabled={busy}
        onclick={() => onclose(n)}
        data-testid="terminal-close-{n}">×</button
      >
    </span>
  {/each}
  <button
    type="button"
    class="act"
    disabled={busy || full}
    title={full ? `${MAX_SHELL_TERMINALS} terminals is the most a session keeps` : `New terminal ${newChord}`}
    onclick={onnew}
    data-testid="terminal-new">+ New <kbd>{newChord}</kbd></button
  >
  <span class="spacer"></span>
  {#if onpopout}
    <button
      type="button"
      class="act"
      title="Open this terminal in a window of its own. It stays here too."
      onclick={onpopout}
      data-testid="terminal-popout">Pop out</button
    >
  {/if}
  {#if active !== null}
    <span class="menu-wrap">
      <button
        type="button"
        class="act"
        aria-haspopup="menu"
        aria-expanded={menuFor === active}
        aria-label="Shell {active} actions"
        title="Shell {active} actions"
        disabled={busy}
        onclick={() => (menuFor === active ? closeMenu() : void openMenu(active as number))}
        data-testid="terminal-menu-toggle">⋯</button
      >
      {#if menuFor !== null && menuFor === active}
        {@const n = menuFor}
        <!-- svelte-ignore a11y_interactive_supports_focus -->
        <div class="menu" role="menu" aria-label="Terminal actions" bind:this={menuEl} onkeydown={menuKey} data-testid="terminal-menu">
          <button type="button" role="menuitem" class="mi" onclick={() => pick(onclear)}>Clear</button>
          <button type="button" role="menuitem" class="mi" onclick={() => pick(onsplit)}
            >{split ? 'Show one terminal' : 'Split right'}</button
          >
          {#if onpopout}
            <button type="button" role="menuitem" class="mi" onclick={() => pick(onpopout)}>Pop out ↗</button>
          {/if}
          <div class="sep" role="separator"></div>
          <button
            type="button"
            role="menuitem"
            class="mi mi--danger"
            onclick={() => {
              // Read now: `n` follows `menuFor`, which closing the menu clears.
              const which = n;
              pick(() => (killAsk = which));
            }}
            data-testid="terminal-kill">Kill terminal…<span class="meta">the session keeps running</span></button
          >
        </div>
      {/if}
    </span>
    <button
      type="button"
      class="act"
      class:on={split}
      aria-pressed={split}
      title={split ? 'Show one terminal' : 'Agent on the left, this shell on the right'}
      onclick={onsplit}
      data-testid="terminal-split-toggle">Split</button
    >
    <button type="button" class="act" title="Clear this shell's screen" onclick={onclear} data-testid="terminal-clear"
      >Clear</button
    >
  {/if}
  {#if host && onopenson}
    <label class="opens-on" title="Where + New starts the next terminal. A terminal always runs on the session's host.">
      <span class="meta">New terminal opens on</span>
      <select
        value={opensOn}
        onchange={(e) => onopenson?.((e.currentTarget as HTMLSelectElement).value as TerminalStart)}
        data-testid="terminal-opens-on"
      >
        {#each TERMINAL_STARTS as at (at)}
          <option value={at}>{terminalStartLabel(at, host)}</option>
        {/each}
      </select>
    </label>
  {/if}
</div>

{#if killAsk !== null}
  {@const n = killAsk}
  <ConfirmDialog
    title="Kill Shell {n}?"
    message="Its tmux session ends and anything running in it stops. The session and its agent keep running."
    confirmLabel="Kill terminal"
    danger
    confirmTestId="terminal-kill-confirm"
    onconfirm={() => {
      const which = n;
      killAsk = null;
      onclose(which);
    }}
    oncancel={() => (killAsk = null)}
  />
{/if}

<style>
  .strip {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    font-size: var(--text-2xs);
    overflow-x: auto;
  }
  .tab-wrap {
    display: inline-flex;
    align-items: center;
    border-radius: var(--radius-sm);
  }
  .tab,
  .act,
  .close {
    font: inherit;
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    border-radius: var(--radius-sm);
    padding: 0.15rem 0.5rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .tab:hover,
  .act:hover:not(:disabled),
  .close:hover:not(:disabled) {
    color: var(--fg);
  }
  .tab.on {
    color: var(--fg);
    border-color: var(--border);
    background: var(--bg);
  }
  .close {
    padding: 0.15rem 0.3rem;
  }
  .act {
    border-color: var(--border);
  }
  .act.on {
    color: var(--fg);
    border-color: var(--accent);
  }
  .act:disabled,
  .close:disabled {
    opacity: 0.5;
    cursor: default;
  }
  kbd {
    font: inherit;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin-left: 0.2rem;
  }
  .spacer {
    flex: 1 1 auto;
  }
  .run {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    margin-left: 0.35rem;
    color: var(--fg-muted);
  }
  .run-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--status-working);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .menu-wrap {
    position: relative;
    display: inline-flex;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 3;
    width: 16rem;
    padding: 0.3rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    height: var(--control-h-lg);
    padding: 0 0.5rem;
    border: none;
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .mi:hover,
  .mi:focus-visible {
    background: var(--accent-soft);
  }
  .mi:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .mi--danger {
    color: var(--status-failed);
  }
  .mi .meta {
    margin-left: auto;
  }
  .sep {
    height: 1px;
    margin: 0.25rem 0;
    background: var(--border);
  }
  .meta {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .opens-on {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    margin-left: 0.25rem;
  }
  .opens-on select {
    font: inherit;
    font-size: var(--text-2xs);
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.1rem 0.3rem;
  }
</style>
