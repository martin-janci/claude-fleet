<script lang="ts">
  // The Inbox's extra fleets (multi-hub Phase 2): per fleet the filter
  // shows, its sessions that need you, worst first, under its badge. A row
  // opens the session in its own view on its own hub.
  import { fleetInbox, fleetSelection, openFleetSession } from './fleets';
  import { sessionStatusWord } from './attention';
</script>

{#each $fleetInbox as g (g.fleet.id)}
  <div class="fleet-inbox" style="--fleet: {g.fleet.color}" data-testid="fleet-inbox-{g.fleet.id}">
    <div class="head">
      <span class="badge">{g.fleet.label}</span>
      <span class="count">{g.rows.length}</span>
    </div>
    {#each g.rows as s (s.id)}
      <button
        type="button"
        class="row"
        class:selected={$fleetSelection?.fleet === g.fleet.id && $fleetSelection?.id === s.id}
        data-testid="fleet-inbox-row-{g.fleet.id}-{s.id}"
        onclick={() => openFleetSession(g.fleet.id, s.id)}
      >
        <span class="name">{s.friendly_name || s.tmux_name}</span>
        <span class="muted">{s.host_alias} · {sessionStatusWord(s)}</span>
      </button>
    {/each}
  </div>
{/each}

<style>
  .fleet-inbox {
    border-left: 3px solid var(--fleet);
    margin: var(--space-2) 0;
    padding-left: var(--space-2);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }
  .badge {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    border: 1px solid var(--fleet);
    background: color-mix(in srgb, var(--fleet) 16%, transparent);
    font-weight: 600;
  }
  .count {
    color: var(--fg-muted);
  }
  .row {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    width: 100%;
    background: none;
    border: 0;
    border-radius: var(--radius-xs);
    padding: 2px var(--space-1);
    font: inherit;
    font-size: var(--text-sm);
    color: var(--fg);
    text-align: left;
    cursor: pointer;
  }
  .row:hover,
  .row.selected {
    background: color-mix(in srgb, var(--fleet) 12%, transparent);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
</style>
