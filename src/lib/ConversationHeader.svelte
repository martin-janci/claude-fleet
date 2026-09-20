<script lang="ts">
  import { untrack } from 'svelte';
  // Sticky bar above the Conversations tab thread: conversation switcher,
  // context meter, model, status and last notable event (spec §6).
  import type { SessionRow } from './sessions';
  import {
    contextMeter,
    conversationTitle,
    relativeTime,
    statusChip,
    switcherEntries,
    type ConversationSummary,
  } from './conversation';
  import type { TurnIndexEntry } from './conversation_nav';
  import { contextColor, contextTint } from './attention';

  // The find and turn-index state lives in ConversationPanel (it owns the
  // thread and the scroller); the header only renders the controls and
  // reports back. Every one of them is optional so the header still stands
  // alone — a panel always passes the whole set.
  let {
    session,
    conversations,
    viewing,
    lastEvent,
    newerAvailable,
    onSelect,
    findOpen = false,
    findDisabled = false,
    findQuery = '',
    findCount = '',
    matchCount = 0,
    turnEntries = [],
    turnsOpen = false,
    nowMs = Date.now(),
    onFindOpen = () => {},
    onFindClose = () => {},
    onFindInput = () => {},
    onFindKey = () => {},
    onFindStep = () => {},
    onTurnsToggle = () => {},
    onPickTurn = () => {},
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
    /** Find bar shown in place of the facts. */
    findOpen?: boolean;
    /** Nothing to search (empty, error or loading thread): the ⌕ button is
     *  disabled rather than hidden, so the tool cluster keeps its shape. */
    findDisabled?: boolean;
    findQuery?: string;
    /** Rendered "n / m" label; '' hides the count. */
    findCount?: string;
    /** Matches found; 0 disables the step buttons. */
    matchCount?: number;
    turnEntries?: TurnIndexEntry[];
    turnsOpen?: boolean;
    /** The panel's ticking clock, for the turn index's relative times. The
     *  default is a fallback for a header rendered on its own; it is read
     *  once, so only a real `nowMs` from the panel ticks. */
    nowMs?: number;
    onFindOpen?: () => void;
    onFindClose?: () => void;
    onFindInput?: (q: string) => void;
    onFindKey?: (e: KeyboardEvent) => void;
    onFindStep?: (d: 1 | -1) => void;
    /** Toggles the turn index; also how the header closes it. */
    onTurnsToggle?: () => void;
    onPickTurn?: (rowKey: string) => void;
  } = $props();

  let findInput: HTMLInputElement | undefined = $state();
  /** ⌘F opens find from the panel, which owns the shortcut; the box it has
   *  to land in lives here. */
  export function focusFindInput(): void {
    findInput?.focus();
    findInput?.select();
  }

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

  // ─── Turn index ───────────────────────────────────────────────────────────
  // Moved here from ConversationPanel so the tool cluster is one bar: the
  // list, its keyboard walk and its outside-pointerdown close are unchanged,
  // except that `turnsOpen` is the panel's state, so closing goes back out
  // through onTurnsToggle (only ever called while it is open).
  let turnsWrap: HTMLDivElement | undefined = $state();
  let turnsList: HTMLUListElement | undefined = $state();
  let turnsButton: HTMLButtonElement | undefined = $state();

  function onTurnsKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onTurnsToggle();
      turnsButton?.focus();
      return;
    }
    // Arrow / Home / End walk the list: a long thread must not need one Tab
    // per turn. The ends hold rather than wrap, so a held arrow stops.
    const rows = Array.from(turnsList?.querySelectorAll<HTMLButtonElement>('button') ?? []);
    if (rows.length === 0) return;
    const at = rows.indexOf(document.activeElement as HTMLButtonElement);
    const to =
      e.key === 'ArrowDown' ? at + 1
      : e.key === 'ArrowUp' ? at - 1
      : e.key === 'Home' ? 0
      : e.key === 'End' ? rows.length - 1
      : null;
    if (to === null) return;
    e.preventDefault();
    rows[Math.max(0, Math.min(to, rows.length - 1))]?.focus();
  }
  $effect(() => {
    if (turnsOpen) turnsList?.querySelector('button')?.focus();
  });
  // Close the list on an outside pointerdown (as the switcher does).
  $effect(() => {
    if (!turnsOpen) return;
    function onDocPointerDown(e: PointerEvent) {
      if (turnsWrap && e.target instanceof Node && !turnsWrap.contains(e.target)) onTurnsToggle();
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
  {#if findOpen}
    <div class="find-inline" data-testid="conv-find">
      <input
        class="field"
        type="search"
        data-testid="conv-find-input"
        aria-label="Find in conversation"
        placeholder="Find in conversation"
        bind:this={findInput}
        value={findQuery}
        oninput={(e) => onFindInput(e.currentTarget.value)}
        onkeydown={onFindKey}
      />
      <span class="tag find-count" data-testid="conv-find-count" aria-live="polite">{findCount}</span>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-prev" aria-label="Previous match" title="Previous match" disabled={matchCount === 0} onclick={() => onFindStep(-1)}>↑</button>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-next" aria-label="Next match" title="Next match" disabled={matchCount === 0} onclick={() => onFindStep(1)}>↓</button>
      <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-close" aria-label="Close find" title="Close find" onclick={onFindClose}>×</button>
    </div>
  {:else}
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
  {/if}

  <!-- Always at the right, find open or not: ⌘F must not relocate the
       pointer target, and "n turns" must not vanish while find is open. -->
  <div class="tools">
    <button type="button" class="btn btn--icon btn--quiet" data-testid="conv-find-button" aria-label="Find in conversation" title="Find (⌘F / Ctrl+F)" disabled={findDisabled} onclick={onFindOpen}>⌕</button>
    {#if turnEntries.length > 0}
      <div class="turns-wrap" bind:this={turnsWrap}>
        <button
          type="button"
          class="btn btn--quiet"
          data-testid="conv-turns-button"
          aria-expanded={turnsOpen}
          bind:this={turnsButton}
          onclick={onTurnsToggle}>{turnEntries.length} turn{turnEntries.length === 1 ? '' : 's'}<span class="caret">▾</span></button
        >
        {#if turnsOpen}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <ul class="turn-index" aria-label="Turns" data-testid="conv-turn-index" bind:this={turnsList} onkeydown={onTurnsKey}>
            {#each turnEntries as t (t.rowKey)}
              <li>
                <button type="button" data-testid="conv-turn-index-item" onclick={() => onPickTurn(t.rowKey)}>
                  <span class="ti-label">{t.label}</span>
                  {#if t.at}<time datetime={t.at}>{relativeTime(t.at, nowMs)}</time>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .conv-header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    min-height: 32px;
    /* One inset expression, shared with .thread and .composer (Task 8). */
    padding: 4px max(1.1rem, calc((100% - var(--chat-col)) / 2 + 1.1rem));
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
  .facts,
  .find-inline {
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    flex: 1 1 auto;
    min-width: 0;
  }
  /* Always at the right, find open or not: no pointer relocation on ⌘F. */
  .tools {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 0 0 auto;
  }
  .find-inline .field {
    flex: 1 1 auto;
    min-width: 0;
    max-width: 40ch;
    height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--fg);
    font: inherit;
    font-size: var(--control-font);
  }
  /* Only :focus-visible gets the 2px ring (controls.css states this as the
     app's single rule); a plain :focus keeps the accent boundary. */
  .find-inline .field:focus {
    outline: none;
    border-color: var(--accent);
  }
  .find-inline .field:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: var(--ring-offset);
  }
  .find-count {
    min-width: 4.5ch;
    font-variant-numeric: tabular-nums;
  }
  .turns-wrap {
    position: relative;
  }
  .turn-index {
    position: absolute;
    right: 0;
    top: calc(100% + 0.25rem);
    z-index: 3;
    width: min(60ch, 90cqw);
    max-height: 22rem;
    overflow: auto;
    overscroll-behavior: contain;
    margin: 0;
    padding: 0.25rem 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--fg) 15%, transparent);
  }
  .turn-index:focus {
    outline: none;
  }
  .turn-index button {
    display: flex;
    width: 100%;
    align-items: baseline;
    gap: 0.75rem;
    padding: 0.3rem 0.65rem;
    border: none;
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: 0.78rem;
    text-align: left;
    cursor: pointer;
  }
  .turn-index button:hover,
  .turn-index button:focus-visible {
    outline: none;
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .ti-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  /* A pane narrow enough that the 1.1rem gutters cost more than they give
     (the rule the old .toolbar carried, now the header's). */
  @container chat (max-width: 34rem) {
    .conv-header {
      padding-inline: 0.6rem;
    }
  }
</style>
