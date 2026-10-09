<script lang="ts">
  /**
   * The session's terminal tabs (redesign step 5.3, Terminals board): the
   * agent's own screen, then shell terminals 1..N, with + New, Split and
   * Clear. Pure presentation: `TerminalView` owns the list and the actions.
   */
  import { MAX_SHELL_TERMINALS } from './terminals';
  import { shortcutLabel } from './shortcuts';
  import { detectMac } from './terminal_keys';

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
  } = $props();

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const newChord = shortcutLabel('new-terminal', isMac);
  const nextChord = shortcutLabel('next-terminal', isMac);
  const full = $derived(shells.length >= MAX_SHELL_TERMINALS);
</script>

<div class="strip" role="tablist" aria-label="Terminals" data-testid="terminal-strip">
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
        title="Shell {n} · next tab {nextChord}"
        onclick={() => onselect(n)}
        data-testid="terminal-tab-{n}">Shell {n}</button
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
</div>

<style>
  .strip {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    font-size: 0.8rem;
    overflow-x: auto;
  }
  .tab-wrap {
    display: inline-flex;
    align-items: center;
    border-radius: 5px;
  }
  .tab,
  .act,
  .close {
    font: inherit;
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    border-radius: 5px;
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
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin-left: 0.2rem;
  }
  .spacer {
    flex: 1 1 auto;
  }
</style>
