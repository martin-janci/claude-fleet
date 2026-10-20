<script lang="ts">
  // The sidebar's fleet filter (multi-hub design): All fleets, or one fleet.
  // Shown only once this window shows more than one fleet, so a one-hub
  // install looks exactly as before. Each chip carries the fleet's colour and
  // a dot for its live link; the word beside the dot says the state, so the
  // colour never carries it alone. An extra fleet's chip counts its sessions
  // that need you (Phase 2), whatever the filter shows.
  import { fleets, fleetFilter, multiFleet, connectionWord, fleetNeedsCount } from './fleets';
  import { radiogroupKeys } from './radiogroup_keys';
</script>

{#if $multiFleet}
  <div class="fleet-switcher" role="radiogroup" aria-label="Fleet" data-testid="fleet-switcher" use:radiogroupKeys>
    <button
      type="button"
      role="radio"
      class="chip"
      aria-checked={$fleetFilter === 'all'}
      data-testid="fleet-filter-all"
      onclick={() => fleetFilter.set('all')}>All fleets</button
    >
    {#each $fleets as f (f.id)}
      {@const cw = connectionWord(f)}
      <button
        type="button"
        role="radio"
        class="chip"
        style="--fleet: {f.color}"
        aria-checked={$fleetFilter === f.id}
        title="{f.label}{f.url ? ` · ${f.url}` : ''} · {cw.word}"
        data-testid="fleet-filter-{f.id}"
        onclick={() => fleetFilter.set(f.id)}
      >
        <span class="swatch" aria-hidden="true"></span>
        {f.label}
        <!-- Review w19-8: the words are text, not an aria-label on a role-less span. -->
        <span class="dot {cw.tone}" aria-hidden="true"></span><span class="sr-only" data-testid="fleet-state-{f.id}">{cw.word}</span>
        {#if ($fleetNeedsCount.get(f.id) ?? 0) > 0}
          <span class="needs" data-testid="fleet-needs-count-{f.id}"
            ><span aria-hidden="true">{$fleetNeedsCount.get(f.id)}</span><span class="sr-only"
              >, {$fleetNeedsCount.get(f.id)} need you</span
            ></span
          >
        {/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .fleet-switcher {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-2) var(--space-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    height: var(--control-h-sm);
    padding: 0 var(--space-2);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-pill);
    background: var(--control-bg);
    color: var(--control-fg);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .chip[aria-checked='true'] {
    background: var(--control-bg-active);
    border-color: var(--control-border-strong);
  }
  .swatch {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-xs);
    background: var(--fleet);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--status-idle);
  }
  .dot.ok {
    background: var(--status-done);
  }
  .dot.warn {
    background: var(--status-waiting);
  }
  .dot.bad {
    background: var(--status-failed);
  }
  .needs {
    min-width: 14px;
    padding: 0 4px;
    border-radius: var(--radius-pill);
    background: var(--status-waiting);
    color: var(--on-waiting);
    font-weight: 600;
    text-align: center;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
