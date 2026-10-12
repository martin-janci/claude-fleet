<script lang="ts">
  // The active-filter strip: one removable chip per filter that narrows the
  // list, and "Clear all". Shown only while something narrows it, so a
  // narrowed list never looks like an empty fleet. The same strip serves the
  // Sessions list and the Work view (facets from filter_facets.ts).
  import { tick } from 'svelte';
  import type { Facet } from './filter_facets';

  let {
    facets,
    onclear,
    onclearall,
    testid = 'active-filters',
    clearAllTestid = 'filters-clear-all',
    emptyFocus,
  }: {
    facets: readonly Facet[];
    onclear: (id: string) => void;
    onclearall: () => void;
    testid?: string;
    clearAllTestid?: string;
    /** Where focus goes when the last chip is removed (the strip is gone):
     *  the bar's Filters button. */
    emptyFocus?: () => HTMLElement | null | undefined;
  } = $props();

  let strip: HTMLDivElement | undefined = $state();
  // The live region stays mounted: inside the strip, clearing the last
  // filter unmounted it with its message, so the change was never read.
  let seen = $state(false);
  $effect(() => {
    if (facets.length > 0) seen = true;
  });
  const announcement = $derived(
    facets.length > 0
      ? `${facets.length} filter${facets.length === 1 ? '' : 's'} active`
      : seen
        ? 'No filters active'
        : '',
  );

  /** Remove a chip without dropping focus to the page: the next chip (the
   *  one that takes its place), else the last, else the bar's fallback. */
  async function remove(id: string, index: number) {
    const hadFocus = strip?.contains(document.activeElement) ?? false;
    onclear(id);
    if (!hadFocus) return;
    await tick();
    const chips = strip ? Array.from(strip.querySelectorAll<HTMLElement>('.facet')) : [];
    const next = chips[Math.min(index, chips.length - 1)];
    if (next) next.focus();
    else emptyFocus?.()?.focus();
  }
  async function removeAll() {
    const hadFocus = strip?.contains(document.activeElement) ?? false;
    onclearall();
    if (!hadFocus) return;
    await tick();
    const first = strip?.querySelector<HTMLElement>('.facet');
    if (first) first.focus();
    else emptyFocus?.()?.focus();
  }
</script>

{#if facets.length > 0}
  <div class="active-filters" role="group" aria-label="Active filters" data-testid={testid} bind:this={strip}>
    {#each facets as f, i (f.id)}
      <button
        type="button"
        class="btn btn--chip facet"
        data-testid="facet-{f.id}"
        aria-label="Remove filter: {f.label}"
        title="Remove this filter"
        onclick={() => remove(f.id, i)}
      >
        <span class="facet-label">{f.label}</span><span class="x" aria-hidden="true">×</span>
      </button>
    {/each}
    {#if facets.length > 1}
      <button type="button" class="btn btn--quiet clear-all" data-testid={clearAllTestid} onclick={removeAll}>Clear all</button>
    {/if}
  </div>
{/if}
<span class="sr-only" aria-live="polite" data-testid="{testid}-status">{announcement}</span>

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
    font-size: var(--text-md);
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
