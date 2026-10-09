<!-- The 68 px left rail (manual: Rail). The consumer gives the items in the
     manual's order (Control, Inbox, Sessions, Work, Automation, Accounts,
     Toolkit; Settings at the bottom), the current one, and the Needs you
     count for the Inbox badge; every title names its shortcut. -->
<script lang="ts">
  import Icon from './Icon.svelte';
  import type { OfIconName } from './icons';

  interface RailEntry {
    id: string;
    label: string;
    icon: OfIconName;
    /** Tooltip: the name and its shortcut ("Control  ⌘E"). */
    title?: string;
    /** Only the Inbox, only the Needs you count. */
    badge?: number;
    /** Sits at the bottom (Settings). */
    bottom?: boolean;
  }

  let {
    items,
    current,
    onselect,
    label = 'Main',
    testid,
  }: { items: RailEntry[]; current?: string; onselect: (id: string) => void; label?: string; testid?: string } = $props();

  const top = $derived(items.filter((i) => !i.bottom));
  const bottom = $derived(items.filter((i) => i.bottom));

  function pick(e: MouseEvent, id: string) {
    e.preventDefault();
    onselect(id);
  }
</script>

{#snippet entry(item: RailEntry)}
  <a
    href="#{item.id}"
    title={item.title ?? item.label}
    aria-current={current === item.id ? 'page' : undefined}
    onclick={(e) => pick(e, item.id)}
    data-rail={item.id}
    data-testid={testid ? `${testid}-${item.id}` : undefined}
    ><Icon name={item.icon} size={18} />{item.label}{#if item.badge}<span
        class="badge tnum"
        aria-label="{item.badge} need you"
        data-testid={testid ? `${item.id}-${testid}-count` : undefined}>{item.badge}</span
      >{/if}</a
  >
{/snippet}

<nav class="of of-rail" aria-label={label} data-testid={testid}>
  {#each top as item (item.id)}{@render entry(item)}{/each}
  <span class="grow"></span>
  {#each bottom as item (item.id)}{@render entry(item)}{/each}
</nav>
