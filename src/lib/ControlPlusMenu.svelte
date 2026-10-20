<!--
  The composer's "+" menu in Control (Thread room parity, "Composer (+, #
  task, @ host, / command…)"): one chip instead of a row of them. Add a
  project (the form in this chat), start a session, and the asks that are
  commands now (/plan, /tidy, /health, /graph) or a # task link. An ask
  only fills the box; nothing is sent until Enter.
-->
<script lang="ts">
  import { composerDrafts, insertIntoComposer } from './conversation';
  import { openChatWizard } from './forms/chat_wizards';
  import { requestNewSessionOnHost } from './app_views';
  import { tick } from 'svelte';

  let { sessionId, host }: { sessionId: number; host: string } = $props();

  let open = $state(false);
  /** The box already holds text: a command only runs from the start of an
   *  empty box, so the command items wait until it is cleared. */
  let typed = $state(false);
  let root: HTMLElement | undefined = $state();
  let chipEl: HTMLButtonElement | undefined = $state();
  let menuEl: HTMLElement | undefined = $state();
  const uid = `cpm-${Math.random().toString(36).slice(2, 8)}`;

  interface Item {
    id: string;
    label: string;
    hint: string;
    run: () => void;
    /** A slash command, which needs an empty box. */
    command?: boolean;
  }

  const fill = (text: string) => () => insertIntoComposer(sessionId, text);

  const ADD: Item[] = $derived([
    { id: 'project', label: 'Add a project', hint: 'A form in this chat', run: () => openChatWizard(sessionId, 'add_project', { from: 'Control' }) },
    { id: 'session', label: `New session on ${host}`, hint: 'Opens New session', run: () => requestNewSessionOnHost(host) },
  ]);
  const ASK: Item[] = $derived([
    { id: 'plan', label: 'Plan a task', hint: '/plan', run: fill('/plan '), command: true },
    { id: 'task', label: 'Link a task', hint: '#', run: fill('#') },
    { id: 'tidy', label: 'Tidy up done tickets', hint: '/tidy', run: fill('/tidy'), command: true },
    { id: 'health', label: 'Host and session health', hint: '/health', run: fill('/health'), command: true },
    { id: 'graph', label: 'Mission task graph', hint: '/graph', run: fill('/graph'), command: true },
  ]);

  const menuItems = () => Array.from(menuEl?.querySelectorAll<HTMLElement>('[role=menuitem]:not(:disabled)') ?? []);

  async function toggle() {
    open = !open;
    if (open) {
      typed = (composerDrafts.get(sessionId) ?? '').trim() !== '';
      // A menu takes focus on its first item (review w19-7), as
      // SessionRowMenu does.
      await tick();
      menuItems()[0]?.focus();
    }
  }

  /** Close from inside the menu and give focus back to "+" before the item
   *  runs (review w19-6): an item that fills the box moves focus on to the
   *  composer itself; one that opens a dialog moves it there. */
  function closeToChip() {
    open = false;
    chipEl?.focus();
  }

  function pick(item: Item) {
    closeToChip();
    item.run();
  }

  function onDocDown(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
  function onKey(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeToChip();
      return;
    }
    if (e.key === 'Tab' && menuEl?.contains(e.target as Node)) {
      // Tab leaves a menu: it closes, and focus moves on from "+".
      open = false;
      chipEl?.focus();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const all = menuItems();
    if (!all.length || !menuEl?.contains(e.target as Node)) return;
    e.preventDefault();
    e.stopPropagation();
    const i = all.indexOf(document.activeElement as HTMLElement);
    const n =
      e.key === 'Home'
        ? 0
        : e.key === 'End'
          ? all.length - 1
          : e.key === 'ArrowDown'
            ? (i + 1) % all.length
            : (i - 1 + all.length) % all.length;
    all[n].focus();
  }
</script>

<svelte:document onpointerdown={onDocDown} />

{#snippet group(key: string, title: string, items: Item[])}
  <!-- Review w19-7: a heading may not sit directly in a menu; each block is
       a group named by its heading. -->
  <div class="group" role="group" aria-labelledby="{uid}-{key}">
    <p class="head" id="{uid}-{key}" role="presentation">{title}</p>
    {#each items as item (item.id)}
      <button
        type="button"
        role="menuitem"
        class="item"
        data-testid="control-plus-{item.id}"
        disabled={item.command && typed}
        title={item.command && typed ? 'Clear the box first: a command runs from an empty box' : undefined}
        onclick={() => pick(item)}
      >
        <span class="label">{item.label}</span>
        <span class="hint">{item.hint}</span>
      </button>
    {/each}
  </div>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span class="wrap" bind:this={root} onkeydown={onKey}>
  <button
    type="button"
    bind:this={chipEl}
    class="btn btn--chip plus"
    data-testid="control-plus"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label="Add or ask"
    title="Add a project, start a session, or put a command in the box"
    onclick={() => void toggle()}>+</button
  >
  {#if open}
    <div class="pop" data-testid="control-plus-menu">
      <div class="menu" role="menu" aria-label="Add or ask" bind:this={menuEl}>
        {@render group('add', 'Add', ADD)}
        {@render group('ask', 'Ask Control', ASK)}
      </div>
      {#if typed}<p class="note" data-testid="control-plus-typed">Clear the box to use a command.</p>{/if}
    </div>
  {/if}
</span>

<style>
  .wrap {
    position: relative;
    display: inline-flex;
  }
  .plus {
    min-width: 28px;
    justify-content: center;
    font-weight: 600;
  }
  .pop {
    position: absolute;
    left: 0;
    bottom: calc(100% + 6px);
    z-index: 20;
    width: 280px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
    box-shadow: var(--shadow-pop);
    color: var(--fg);
  }
  .menu,
  .group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .head {
    margin: var(--space-1) var(--space-2) 2px;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .group:first-child .head {
    margin-top: 0;
  }
  .item {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    padding: 6px var(--space-2);
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .item:disabled {
    color: var(--fg-muted);
    cursor: default;
  }
  .note {
    margin: var(--space-1) var(--space-2) 0;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .item:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .item:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -2px;
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
  }
</style>
