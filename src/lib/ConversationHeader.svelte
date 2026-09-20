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
    type BackgroundEntry,
    type ConversationSummary,
  } from './conversation';
  import type { TurnIndexEntry } from './conversation_nav';
  import { contextColor } from './attention';

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
    bgGroups = [],
    backgroundOpen = false,
    onBackgroundToggle = () => {},
    onPickBackground = () => {},
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
    /** Background work the conversation launched, already grouped by the
     *  panel (a group with no entries is not passed). Empty hides the
     *  control, as an empty `turnEntries` hides the turn index. */
    bgGroups?: Array<{ title: string; entries: BackgroundEntry[] }>;
    backgroundOpen?: boolean;
    /** Toggles the background list; also how the header closes it. */
    onBackgroundToggle?: () => void;
    onPickBackground?: (entry: BackgroundEntry) => void;
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

  // ─── Background switcher ──────────────────────────────────────────────────
  // Third control of the tool cluster, beside ⌕ and the turn index, rather
  // than the second sticky bar this branch removed. The panel owns the
  // entries and what opening one means; the header only renders the
  // disclosure and reports the pick back.
  let bgWrap: HTMLDivElement | undefined = $state();
  // The count on the button and the gate on the whole control: the panel's
  // flat list, re-added rather than passed twice.
  const bgCount = $derived(bgGroups.reduce((n, g) => n + g.entries.length, 0));
  $effect(() => {
    if (!backgroundOpen) return;
    function onDocPointerDown(e: PointerEvent) {
      if (bgWrap && e.target instanceof Node && !bgWrap.contains(e.target)) onBackgroundToggle();
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
        style="color: {contextColor(meter.level)}; border-color: {contextColor(meter.level)};"
        ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, meter.pct))}%; background: {contextColor(meter.level)};"></span><span class="ctx-pct">{meter.label}</span></span
      >
    {/if}
    {#if model}<span class="tag tag--mono" data-testid="conv-model">{model}</span>{/if}
    {#if status}<span class="tag" data-testid="conv-status" data-status={status}>{status}</span>{/if}
    {#if lastEvent}<span class="tag last-event" data-testid="conv-last-event">{lastEvent}</span>{/if}
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
    {#if bgCount > 0}
      <div class="turns-wrap" bind:this={bgWrap}>
        <button
          type="button"
          class="btn btn--quiet"
          data-testid="conv-background-button"
          aria-expanded={backgroundOpen}
          onclick={onBackgroundToggle}>{bgCount} background<span class="caret">▾</span></button
        >
        {#if backgroundOpen}
          <div class="turn-index bg-groups" data-testid="conv-background-list">
            {#each bgGroups as g (g.title)}
              <div class="bg-group" data-testid="conv-background-group">{g.title}</div>
              <ul aria-label={g.title}>
                {#each g.entries as e (e.key)}
                  <li>
                    <button type="button" data-testid="conv-background-item" onclick={() => onPickBackground(e)}>
                      <span class="ti-label">{e.kind} · {e.label}</span>
                      <span class="bg-item-status" data-status={e.status}>{e.status}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .conv-header {
    position: sticky;
    top: 0;
    /* Above the thread's own sticky bars (the toolbar / find row, z-index 2)
       and the turn index they open (3): both are later in the DOM, so an
       equal z-index would let them paint over the switcher menu that drops
       out of this header. */
    z-index: 4;
    display: flex;
    align-items: center;
    gap: var(--control-gap);
    min-height: 32px;
    /* One inset expression, shared with .thread and .composer (Task 8). */
    padding: 4px var(--chat-inset);
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
    /* The row itself never wraps and .switcher-wrap/.tools guard their own
       widths; this is the backstop that keeps the non-elastic tags (.ctx,
       the model/status tags) from pushing the pane sideways once the
       elastic .last-event tag has already shrunk to nothing. */
    overflow: hidden;
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
  .turn-index button:hover {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  /* The shared ring (controls.css), not `outline: none` plus a ~1.1:1 tint:
     a tint that faint is not a focus indicator, and suppressing the outline
     left keyboard users with nothing. Drawn inward because the list clips. */
  .turn-index button:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .ti-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The grouped dropdown keeps `.turn-index`'s popup chrome; the inner lists
     shed the browser's own list styling. */
  .bg-groups ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .bg-group {
    padding: 0.3rem 0.65rem 0.15rem;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .bg-group:not(:first-child) {
    margin-top: 0.25rem;
    border-top: 1px solid var(--border);
    padding-top: 0.4rem;
  }
  .bg-item-status {
    margin-left: auto;
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .bg-item-status[data-status='failed'] {
    color: var(--usage-crit);
  }
  .bg-item-status[data-status='stopped'] {
    color: var(--usage-warn);
  }
  .tag[data-status='compacting'],
  .tag[data-status='blocked'] {
    color: var(--usage-warn);
  }
  .tag[data-status='failed'] {
    color: var(--usage-crit);
  }
  .tag.last-event {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* A hairline divider instead of two competing pill borders. */
  .facts .tag + .tag::before {
    content: '';
    width: 1px;
    height: 11px;
    margin-right: var(--control-gap);
    background: var(--control-border);
  }
  .ctx {
    position: relative;
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
    height: 18px;
    padding: 0 7px;
    border: 1px solid;
    border-radius: var(--radius-pill);
    overflow: hidden;
    font-size: var(--control-font-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .ctx[data-stale='true'] {
    opacity: 0.55;
  }
  .ctx-bar {
    position: absolute;
    inset: 0 auto 0 0;
    opacity: 0.22;
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
  /* The model tag is the lowest-value fact (status and the context meter
     matter more, last-event already truncates itself) — drop it before the
     row is narrow enough to need .facts' overflow:hidden backstop above. */
  @container chat (max-width: 26rem) {
    .facts .tag--mono {
      display: none;
    }
  }
</style>
