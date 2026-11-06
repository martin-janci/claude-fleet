<script lang="ts">
  // Shown where this window's own rows drop out because the sidebar's fleet
  // filter names another fleet. The filter is remembered and the sidebar is
  // often not on screen (the Hosts page opens from Accounts), so the note
  // names the fleet it is narrowed to and clears the filter in one press
  // (Martin, 2026-11-05: "All hosts" showed only Papaya POS, no way back).
  import { fleetFilter, fleets } from './fleets';
  import Button from './kit/Button.svelte';

  let { testid }: { testid: string } = $props();
  const shown = $derived($fleets.find((f) => f.id === $fleetFilter)?.label ?? $fleetFilter);
</script>

<p class="note" data-testid={testid}>
  <span>This window's own hosts are hidden: the fleet filter in the sidebar shows only {shown}.</span>
  <Button size="sm" variant="quiet" testid="{testid}-show-all" onclick={() => fleetFilter.set('all')}>Show all fleets</Button>
</p>

<style>
  .note {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 1rem;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
</style>
