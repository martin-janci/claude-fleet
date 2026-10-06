<script module lang="ts">
  export interface PickerItem {
    /** Stable identity; also the `data-key` on the row. */
    key: string;
    label: string;
    /** Secondary line (project · host · branch …). */
    description?: string;
    /** Trailing hint (status, "⌘↵"). */
    meta?: string;
    /** A small leading badge (a ticket's tracker), its name in the tooltip. */
    badge?: { icon: string; title: string };
    /** Non-selectable group heading rendered before this row. */
    group?: string;
    testid?: string;
    /** Dimmed (a dormant project): still pickable. */
    dim?: boolean;
    /** A small pill before the meta ("current session", "on mefistos"). */
    chip?: string;
    /** A shortcut hint after the meta ("⌘1"). */
    kbd?: string;
    /** Identity of the group heading, reported by `ongroupclick`. */
    groupKey?: string;
    /** A muted subtitle after the group heading. */
    groupSub?: string;
    /** Render `rowActions` on this row (hover; mouse only). */
    actionable?: boolean;
  }

  /** DOM id of the option for `key` in the list `listId` (for aria-activedescendant). */
  export function optionId(listId: string, key: string): string {
    return `${listId}-opt-${key.replace(/[^A-Za-z0-9_-]/g, '_')}`;
  }
</script>

<script lang="ts">
  // A bounded, scrollable list of pickable rows with an "active" row that is
  // always scrolled into view. Shared by the quick switcher and the
  // new-session dialog's worktree picker; the Sidebar can adopt it after
  // #46 lands. Keyboard handling stays with the OWNER (the switcher's input,
  // the dialog) — this component only renders, scrolls and reports clicks,
  // so the same list works whether focus sits in a search box or on the
  // list itself.
  import { tick, type Snippet } from 'svelte';

  let {
    items,
    activeKey = null,
    onactivate,
    onpick,
    maxHeight = '18rem',
    emptyText = 'Nothing matches.',
    ariaLabel,
    listId,
    testid,
    rowActions,
    ongroupclick,
    oncontext,
  }: {
    items: readonly PickerItem[];
    activeKey?: string | null;
    /** Hover / focus moved the highlight (not a selection). */
    onactivate?: (key: string) => void;
    /** Click on a row. */
    onpick: (key: string) => void;
    maxHeight?: string;
    emptyText?: string;
    ariaLabel?: string;
    /** DOM id of the listbox; also prefixes option ids (see `optionId`). */
    listId?: string;
    testid?: string;
    /** Hover actions for `actionable` rows; mouse only (aria-hidden, not focusable). */
    rowActions?: Snippet<[PickerItem]>;
    /** Click on a group heading that carries a `groupKey`. */
    ongroupclick?: (groupKey: string) => void;
    /** Right-click on a row (the browser's menu is suppressed when set). */
    oncontext?: (key: string, e: MouseEvent) => void;
  } = $props();

  let root: HTMLElement | undefined = $state();

  // Whenever the highlight moves, keep it visible — the reason this
  // component exists. `block: 'nearest'` avoids jumping the list when the
  // row is already on screen. jsdom has no scrollIntoView; guard it.
  $effect(() => {
    const key = activeKey;
    if (!root || key === null) return;
    void tick().then(() => {
      const el = root?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
      if (el && typeof el.scrollIntoView === 'function') {
        el.scrollIntoView({ block: 'nearest' });
      }
    });
  });
</script>

<div
  class="picker-list"
  role="listbox"
  id={listId}
  aria-label={ariaLabel}
  style:max-height={maxHeight}
  bind:this={root}
  data-testid={testid}
>
  {#if items.length === 0}
    <p class="empty">{emptyText}</p>
  {/if}
  {#each items as item, i (item.key)}
    {#if item.group && (i === 0 || items[i - 1].group !== item.group)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="group"
        class:clickable={!!ongroupclick && !!item.groupKey}
        role="presentation"
        onclick={() => item.groupKey && ongroupclick?.(item.groupKey)}
      >{item.group}{#if item.groupSub}<span class="gsub">{item.groupSub}</span>{/if}</div>
    {/if}
    <div
      class="row"
      class:active={item.key === activeKey}
      class:dim={item.dim}
      role="option"
      id={listId ? optionId(listId, item.key) : undefined}
      aria-selected={item.key === activeKey}
      tabindex="-1"
      data-key={item.key}
      data-testid={item.testid}
      onmousemove={() => onactivate?.(item.key)}
      onclick={() => onpick(item.key)}
      onkeydown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onpick(item.key);
        }
      }}
      oncontextmenu={(e) => {
        if (!oncontext) return;
        e.preventDefault();
        oncontext(item.key, e);
      }}
    >
      <div class="main">
        <span class="label"
          >{#if item.badge}<span class="badge" title={item.badge.title} data-testid="picker-badge"
              >{item.badge.icon}</span
            >{/if}{item.label}</span
        >
        {#if item.description}
          <span class="desc">{item.description}</span>
        {/if}
      </div>
      {#if item.chip}<span class="chip">{item.chip}</span>{/if}
      {#if item.meta || item.kbd}
        <span class="meta">{item.meta ?? ''}{#if item.kbd}<kbd class="kbd">{item.kbd}</kbd>{/if}</span>
      {/if}
      {#if item.actionable && rowActions}
        <span class="acts" aria-hidden="true">{@render rowActions(item)}</span>
      {/if}
    </div>
  {/each}
</div>

<style>
  .picker-list {
    overflow-y: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
  }
  .empty {
    margin: 0;
    padding: 0.5rem 0.6rem;
    color: var(--fg-muted);
    font-size: 0.8rem;
  }
  .group {
    padding: 0.35rem 0.6rem 0.1rem;
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .group.clickable {
    cursor: pointer;
  }
  .group.clickable:hover {
    color: var(--fg);
  }
  .gsub {
    margin-left: 0.5rem;
    text-transform: none;
    letter-spacing: 0;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.6rem;
    cursor: pointer;
    font-size: 0.85rem;
    border-left: 2px solid transparent;
  }
  .row.active {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-left-color: var(--accent);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1 1 auto;
  }
  .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .desc {
    font-size: 0.7rem;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    flex-shrink: 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .row.dim .label {
    color: var(--fg-muted);
  }
  .chip {
    flex-shrink: 0;
    font-size: 0.7rem;
    padding: 0.05rem 0.45rem;
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .kbd {
    margin-left: 0.4rem;
    font: inherit;
    font-size: 0.65rem;
    padding: 0 0.25rem;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .acts {
    display: none;
    position: absolute;
    right: 0.4rem;
    top: 50%;
    transform: translateY(-50%);
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    border: 1px solid var(--border);
  }
  .row:hover .acts {
    display: flex;
  }
  .row:hover .meta {
    visibility: hidden;
  }
  .badge {
    font-size: 0.62rem;
    font-weight: 600;
    padding: 0 0.25rem;
    margin-right: 0.35rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    opacity: 0.8;
  }
</style>
