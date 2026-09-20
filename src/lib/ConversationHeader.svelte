<script lang="ts">
  import { untrack } from 'svelte';
  // Sticky bar above the Conversations tab thread: conversation switcher,
  // context meter, model, status and last notable event (spec §6 / Task 3).
  import type { SessionRow } from './sessions';
  import {
    contextMeter,
    conversationTitle,
    statusChip,
    switcherEntries,
    type ConversationSummary,
  } from './conversation';
  import { contextColor, contextTint } from './attention';

  let {
    session,
    conversations,
    viewing,
    lastEvent,
    newerAvailable,
    onSelect,
  }: {
    session: SessionRow;
    conversations: ConversationSummary[];
    /** claude_session_id being viewed; null = the current conversation. */
    viewing: string | null;
    /** From lastEventLabel(); null hides the slot. */
    lastEvent: string | null;
    /** A newer conversation started while an earlier one is being viewed. */
    newerAvailable: boolean;
    onSelect: (claudeSessionId: string | null) => void;
  } = $props();

  let open = $state(false);
  let menu: HTMLUListElement | undefined = $state();
  let wrap: HTMLDivElement | undefined = $state();
  let button: HTMLButtonElement | undefined = $state();

  const entries = $derived(switcherEntries(conversations));
  const shown = $derived(
    viewing === null
      ? entries.find((c) => c.current)
      : entries.find((c) => c.claude_session_id === viewing),
  );
  const title = $derived(shown ? conversationTitle(shown) : viewing === null ? 'Current' : 'Earlier conversation');
  const meter = $derived(viewing === null ? contextMeter(session) : null);
  // An earlier conversation shows its own model; the current one prefers the
  // row's live value.
  const model = $derived(
    ((viewing === null ? (session.model ?? shown?.model) : (shown?.model ?? session.model)) ?? '').replace(/^claude-/, ''),
  );
  const status = $derived(viewing === null ? statusChip(session) : null);

  function pick(c: ConversationSummary) {
    open = false;
    onSelect(c.current ? null : c.claude_session_id);
  }
  /** The entry highlighted and aria-selected: the viewed conversation, or
   *  the current one when viewing it. */
  const isSelected = (c: ConversationSummary) =>
    viewing === null ? c.current : c.claude_session_id === viewing;
  function onItemKey(e: KeyboardEvent, c: ConversationSummary) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      pick(c);
    }
  }
  // Roving focus over the options: a listbox is walked with the arrows, not
  // by tabbing through every past conversation.
  let focusIndex = $state(0);
  let items = $state<Array<HTMLLIElement | undefined>>([]);
  function moveFocus(to: number) {
    if (entries.length === 0) return;
    focusIndex = Math.max(0, Math.min(to, entries.length - 1));
    items[focusIndex]?.focus();
  }
  function onMenuKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      open = false;
      button?.focus();
      return;
    }
    const to =
      e.key === 'ArrowDown' ? focusIndex + 1
      : e.key === 'ArrowUp' ? focusIndex - 1
      : e.key === 'Home' ? 0
      : e.key === 'End' ? entries.length - 1
      : null;
    if (to === null) return;
    e.preventDefault();
    moveFocus(to);
  }
  $effect(() => {
    if (!open) return;
    untrack(() => {
      // Open on the entry the user is looking at, not blindly on the first.
      const at = entries.findIndex(isSelected);
      focusIndex = at === -1 ? 0 : at;
      // No entries yet: the list itself takes focus so Escape still lands.
      if (entries.length === 0) menu?.focus();
      else items[focusIndex]?.focus();
    });
  });
  // Close the menu on an outside pointerdown.
  $effect(() => {
    if (!open) return;
    function onDocPointerDown(e: PointerEvent) {
      if (wrap && e.target instanceof Node && !wrap.contains(e.target)) open = false;
    }
    document.addEventListener('pointerdown', onDocPointerDown);
    return () => document.removeEventListener('pointerdown', onDocPointerDown);
  });
</script>

<div class="conv-header" data-testid="conv-header">
  <div class="switcher-wrap" bind:this={wrap}>
    <button
      type="button"
      class="switcher"
      data-testid="conv-switcher"
      aria-haspopup="listbox"
      aria-expanded={open}
      bind:this={button}
      onclick={() => (open = !open)}
      >{title}{#if newerAvailable}<span class="dot" role="img" aria-label="newer conversation available" data-testid="conv-switcher-dot" title="A newer conversation started"></span>{/if}<span class="caret">▾</span></button
    >
    {#if open}
      <ul class="menu" role="listbox" tabindex="-1" data-testid="conv-switcher-menu" bind:this={menu} onkeydown={onMenuKey}>
        {#each entries as c, i (c.id)}
          <li
            role="option"
            aria-selected={isSelected(c)}
            class:selected={isSelected(c)}
            data-testid="conv-switcher-item"
            data-current={c.current}
            bind:this={items[i]}
            onclick={() => pick(c)}
            onkeydown={(e) => onItemKey(e, c)}
            tabindex={i === focusIndex ? 0 : -1}
          >
            <span class="t">{conversationTitle(c)}</span>
            {#if c.first_prompt}<span class="p">{c.first_prompt}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
  <div class="facts">
    {#if meter}
      <span
        class="ctx"
        data-testid="conv-ctx"
        data-level={meter.level}
        data-stale={meter.stale}
        role="meter"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.round(meter.pct)}
        aria-label="context usage"
        title={meter.title}
        style="color: {contextColor(meter.level)}; border-color: {contextTint(meter.level)};"
        ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, meter.pct))}%; background: {contextColor(meter.level)};"></span><span class="ctx-pct">{meter.label}</span></span
      >
    {/if}
    {#if model}<span class="chip" data-testid="conv-model">{model}</span>{/if}
    {#if status}<span class="chip" data-testid="conv-status" data-status={status}>{status}</span>{/if}
    {#if lastEvent}<span class="muted" data-testid="conv-last-event">{lastEvent}</span>{/if}
  </div>
</div>

<style>
  .conv-header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.4rem 1.1rem;
    background: var(--bg-pane);
    border-bottom: 1px solid var(--border);
  }
  .switcher-wrap {
    position: relative;
    flex: 0 1 auto;
    min-width: 0;
  }
  .switcher {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    max-width: 42ch;
    padding: 0.25rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--fg);
    font-size: 0.8rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .switcher:hover {
    border-color: var(--accent);
  }
  .caret {
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
  }
  .menu {
    position: absolute;
    top: calc(100% + 0.25rem);
    left: 0;
    z-index: 3;
    min-width: 280px;
    /* Never wider than the pane the chat is in (see the inline-size
       container on .conversation-panel), however wide the window is. The
       height stays on the viewport: an inline-size container answers no
       block-axis query, so cqh there would silently mean vh anyway. */
    max-width: 90cqw;
    max-height: 50vh;
    overflow: auto;
    overscroll-behavior: contain;
    margin: 0;
    padding: 0.25rem 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    box-shadow: 0 4px 16px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .menu li {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.35rem 0.65rem;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .menu li:hover,
  .menu li.selected {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .menu .t {
    color: var(--fg);
  }
  .menu .p {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-muted);
    font-size: 0.74rem;
  }
  .facts {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    flex: 1 1 auto;
    min-width: 0;
  }
  .chip {
    padding: 0.1rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg);
    color: var(--fg-muted);
    font-size: 0.72rem;
    white-space: nowrap;
  }
  .chip[data-status='compacting'],
  .chip[data-status='blocked'] {
    color: var(--usage-warn);
  }
  .chip[data-status='failed'] {
    color: var(--usage-crit);
  }
  .muted {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .ctx {
    position: relative;
    display: inline-block;
    flex: 0 0 auto;
    padding: 0.1rem 0.45rem;
    border: 1px solid;
    border-radius: 999px;
    overflow: hidden;
    font-size: 0.68rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .ctx[data-stale='true'] {
    opacity: 0.55;
  }
  .ctx-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    opacity: 0.25;
  }
  .ctx-pct {
    position: relative;
  }
  @media (max-width: 520px) {
    .conv-header {
      flex-wrap: wrap;
    }
    .facts {
      justify-content: flex-start;
    }
  }
</style>
