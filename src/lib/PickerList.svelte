<script module lang="ts">
  export interface PickerItem {
    /** Stable identity; also the `data-key` on the row. */
    key: string;
    label: string;
    /** Secondary line (project · host · branch …). */
    description?: string;
    /** Trailing hint (status, "⌘↵"). */
    meta?: string;
    /** Non-selectable group heading rendered before this row. */
    group?: string;
    testid?: string;
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
  import { tick } from 'svelte';

  let {
    items,
    activeKey = null,
    onactivate,
    onpick,
    maxHeight = '18rem',
    emptyText = 'Nothing matches.',
    ariaLabel,
    testid,
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
    testid?: string;
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
      <div class="group" role="presentation">{item.group}</div>
    {/if}
    <div
      class="row"
      class:active={item.key === activeKey}
      role="option"
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
    >
      <div class="main">
        <span class="label">{item.label}</span>
        {#if item.description}
          <span class="desc">{item.description}</span>
        {/if}
      </div>
      {#if item.meta}
        <span class="meta">{item.meta}</span>
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
  .row {
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
</style>
