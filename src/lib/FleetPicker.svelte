<script lang="ts">
  // The Fleet picker of a per-fleet Settings page (Environment, Company):
  // which fleet's hub the page reads and writes. Shown only while the window
  // has more than one fleet; a fleet removed while picked falls back to the
  // primary. The page routes by the value (`fleet` on a FLEET_SCOPED
  // command, multi-hub design D6).
  import { extraFleets, fleetChoices, fleets, PRIMARY } from './fleets';

  let {
    value = $bindable(PRIMARY),
    testid,
  }: {
    /** The picked fleet: the primary, or an extra fleet's slot id. */
    value?: string;
    testid: string;
  } = $props();

  const choices = $derived(fleetChoices($fleets));
  $effect(() => {
    if (value !== PRIMARY && $fleets.length > 0 && !choices.some((f) => f.id === value)) value = PRIMARY;
  });
</script>

{#if $extraFleets.length > 0}
  <label class="fleet-pick">
    <span>Fleet</span>
    <select data-testid={testid} {value} onchange={(e) => (value = e.currentTarget.value)}>
      {#each choices as f (f.id)}
        <option value={f.id}>{f.primary ? `${f.label} (this window’s own)` : f.label}</option>
      {/each}
    </select>
  </label>
{/if}

<style>
  .fleet-pick {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
