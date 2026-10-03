<script lang="ts">
  // One slot in a hand-built screen (declarative pages L8): draws the items
  // the slot's embed page holds (`embeds.ts`), in spec order, with the
  // context the owning screen hands it. No wrapper element, so a slot sits in
  // its owner's layout exactly where the owner put it; a slot no page fills
  // draws nothing.
  import AccountUsageItem from './usage/AccountUsageItem.svelte';
  import { embedItems } from './embeds';
  import type { Slot } from './pages';
  import type { UsageContext } from './usage/context';

  let { slot, ctx }: { slot: Slot; ctx: UsageContext } = $props();

  const items = $derived(embedItems(slot));
</script>

{#each items as item, i (i)}
  {#if item.type === 'account_usage'}
    <AccountUsageItem view={item.view} {ctx} />
  {/if}
{/each}
