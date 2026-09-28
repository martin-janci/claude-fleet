<script lang="ts">
  // The active-filter strip: one removable chip per filter that narrows the
  // list, and "Clear all". Shown only while something narrows it, so a
  // narrowed list never looks like an empty fleet. The same strip serves the
  // Sessions list and the Work view (facets from filter_facets.ts).
  import type { Facet } from './filter_facets';

  let {
    facets,
    onclear,
    onclearall,
    testid = 'active-filters',
    clearAllTestid = 'filters-clear-all',
  }: {
    facets: readonly Facet[];
    onclear: (id: string) => void;
    onclearall: () => void;
    testid?: string;
    clearAllTestid?: string;
  } = $props();
</script>

{#if facets.length > 0}
  <div class="active-filters" role="group" aria-label="Active filters" data-testid={testid}>
    {#each facets as f (f.id)}
      <button
        type="button"
        class="btn btn--chip facet"
        data-testid="facet-{f.id}"
        aria-label="Remove filter: {f.label}"
        title="Remove this filter"
        onclick={() => onclear(f.id)}
      >
        <span class="facet-label">{f.label}</span><span class="x" aria-hidden="true">×</span>
      </button>
    {/each}
    {#if facets.length > 1}
      <button type="button" class="btn btn--quiet clear-all" data-testid={clearAllTestid} onclick={onclearall}>Clear all</button>
    {/if}
    <span class="sr-only" aria-live="polite">{facets.length} filter{facets.length === 1 ? '' : 's'} active</span>
  </div>
{/if}

<style>
  .active-filters {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
  }
  .facet {
    max-width: 100%;
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--control-fg);
    gap: 6px;
  }
  .facet-label {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .x {
    color: var(--control-fg-quiet);
    font-size: 14px;
  }
  .facet:hover .x {
    color: var(--control-fg);
  }
  .clear-all {
    text-decoration: underline;
    text-underline-offset: 2px;
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
