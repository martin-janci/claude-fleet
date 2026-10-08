<!--
  The New layout's rail (Orbit Fleet redesign step 3.2, design manual's Rail
  component). The order and which items show live in `rail.ts`; this file
  draws them and says which one is current. Every title names its shortcut,
  and the shortcuts themselves stay where they were (`shortcuts.ts`).
-->
<script lang="ts">
  import { destination } from './destination';
  import { sidebarView } from './work_view';
  import { settingsOpen } from './app_views';
  import { currentRailItem, visibleRailItems, type RailId, type RailItem } from './rail';
  import { shortcutLabel } from './shortcuts';
  import { inboxCount } from './inbox';

  interface Props {
    isMac: boolean;
    onselect: (id: RailId) => void;
  }
  let { isMac, onselect }: Props = $props();

  const items = visibleRailItems();
  const top = items.filter((i) => i.id !== 'settings');
  const bottom = items.filter((i) => i.id === 'settings');

  const current = $derived($settingsOpen ? 'settings' : currentRailItem($destination, $sidebarView));

  const TITLE_NAME: Partial<Record<RailId, string>> = { accounts: 'Accounts & hosts' };

  function title(item: RailItem): string {
    const name = TITLE_NAME[item.id] ?? item.label;
    return item.shortcut ? `${name}  ${shortcutLabel(item.shortcut, isMac)}` : name;
  }
</script>

{#snippet icon(id: RailId)}
  <svg class="ico" width="18" height="18" viewBox="0 0 16 16" aria-hidden="true">
    {#if id === 'control'}
      <circle cx="8" cy="8" r="5.5" /><circle cx="8" cy="8" r="2" /><path d="M8 1v2.5M8 12.5V15M1 8h2.5M12.5 8H15" />
    {:else if id === 'inbox'}
      <path d="M2 9.5h3.5l1 1.5h3l1-1.5H14" /><path d="M3.5 3h9L14 9.5V13H2V9.5z" />
    {:else if id === 'sessions'}
      <rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M4.5 6.5 6.5 8l-2 1.5M8 10h3" />
    {:else if id === 'work'}
      <rect x="2.5" y="2.5" width="11" height="11" rx="2" /><path d="m5.5 8 1.8 1.8L10.8 6" />
    {:else if id === 'automation'}
      <path d="M13 8a5 5 0 1 1-1.5-3.6" /><path d="M13 2.5v2.5h-2.5" /><path d="M8 5.5V8l1.8 1.2" />
    {:else if id === 'accounts'}
      <circle cx="8" cy="5.5" r="2.5" /><path d="M3 13.5c.6-2.6 2.6-4 5-4s4.4 1.4 5 4" />
    {:else if id === 'toolkit'}
      <path d="M9.5 2.5a3 3 0 0 0-3 4L2.5 10.5l3 3 4-4a3 3 0 0 0 4-3l-2 1-2-2z" />
    {:else}
      <circle cx="8" cy="8" r="2" /><path
        d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4"
      />
    {/if}
  </svg>
{/snippet}

{#snippet entry(item: RailItem)}
  <button
    type="button"
    class="item"
    aria-current={current === item.id ? 'page' : undefined}
    title={title(item)}
    onclick={() => onselect(item.id)}
    data-testid="rail-{item.id}"
  >
    {@render icon(item.id)}
    <span>{item.label}</span>
    {#if item.id === 'inbox' && $inboxCount > 0}
      <span class="count count-badge count-badge--hot" data-testid="inbox-rail-count" title="{$inboxCount} need you"
        >{$inboxCount}</span
      >
    {/if}
  </button>
{/snippet}

<nav class="rail" aria-label="Main" data-testid="rail">
  {#each top as item (item.id)}
    {@render entry(item)}
  {/each}
  <span class="grow"></span>
  {#each bottom as item (item.id)}
    {@render entry(item)}
  {/each}
</nav>

<style>
  .rail {
    width: var(--rail-w);
    min-width: 0;
    height: 100%;
    box-sizing: border-box;
    padding: 8px 6px;
    border-right: 1px solid var(--border);
    background: var(--bg-pane);
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
  }
  .item {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 8px 0;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: 11px;
    line-height: 14px;
    cursor: pointer;
  }
  .item:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .item[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .ico {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    flex: none;
  }
  /* The Inbox's Needs you count (0.7's hot count badge) on the icon's corner. */
  .count {
    position: absolute;
    top: 3px;
    right: 6px;
  }
  .grow {
    flex: 1 1 auto;
  }
</style>
