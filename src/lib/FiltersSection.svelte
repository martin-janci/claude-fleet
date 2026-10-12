<script lang="ts" generics="G extends string">
  // The left list's Filters section (Orbit Fleet redesign
  // step 3.7), the same for the Sessions list and the Work view: closed, it
  // is one row (search · Filters · Group); open, the panel under it holds
  // every other control, under the headings of `FILTER_SECTIONS`
  // (filter_schema.ts). The owner draws the controls inside the panel and
  // the active-filter strip under the section; this file draws the shape.
  import type { Snippet } from 'svelte';
  import type { GroupOption } from './filter_schema';

  let {
    search,
    onsearch,
    searchLabel,
    placeholder,
    searchTestid,
    count,
    filtersTitle,
    filtersTestid,
    panelId,
    panelLabel,
    panelTestid,
    clearTestid,
    doneTestid,
    groupValue,
    groupOptions,
    ongroup,
    groupTestid,
    onclearall,
    open = $bindable(false),
    filtersBtn = $bindable(),
    trailing,
    panel,
  }: {
    search: string;
    onsearch: (v: string) => void;
    searchLabel: string;
    placeholder: string;
    searchTestid: string;
    /** Active filters the panel holds; the button's badge. */
    count: number;
    filtersTitle: string;
    filtersTestid: string;
    panelId: string;
    panelLabel: string;
    panelTestid: string;
    clearTestid: string;
    doneTestid: string;
    groupValue: G;
    groupOptions: readonly GroupOption<G>[];
    ongroup: (id: G) => void;
    groupTestid: string;
    onclearall: () => void;
    open?: boolean;
    filtersBtn?: HTMLButtonElement;
    /** After Group on the row (the Sessions list's ⋯). */
    trailing?: Snippet;
    panel: Snippet;
  } = $props();

  function close() {
    open = false;
    filtersBtn?.focus();
  }
</script>

<div class="filters-section" data-testid="filters-section">
  <!-- UX audit L3: the search takes the full width; Filters, Group and the
       owner's trailing controls share the line under it. -->
  <div class="fs-head" data-testid="filters-row">
  <input
      class="search"
      type="search"
      {placeholder}
      aria-label={searchLabel}
      value={search}
      oninput={(e) => onsearch((e.currentTarget as HTMLInputElement).value)}
      data-testid={searchTestid}
    />
  <div class="fs-row">
    <button
      bind:this={filtersBtn}
      type="button"
      class="btn btn--quiet is-bounded filters-btn"
      class:has-active={count > 0}
      data-testid={filtersTestid}
      aria-expanded={open}
      aria-controls={panelId}
      aria-label={count > 0 ? `Filters, ${count} active` : 'Filters'}
      title={filtersTitle}
      onclick={() => (open = !open)}
    >
      <span aria-hidden="true">⏷</span> Filters{#if count > 0}<span class="badge">{count}</span>{/if}
    </button>
    <label class="group-wrap" title="Group by">
    <span class="group-label" aria-hidden="true">Group:</span>
    <select
      class="group"
      aria-label="Group by"
      title="Group by"
      data-testid={groupTestid}
      value={groupValue}
      onchange={(e) => ongroup((e.currentTarget as HTMLSelectElement).value as G)}
    >
      {#each groupOptions as o (o.id)}
        <option value={o.id} title={o.title}>{o.label}</option>
      {/each}
    </select>
    </label>
    <span class="spacer"></span>
    {@render trailing?.()}
  </div>
  </div>
  {#if open}
    <div class="fs-panel" id={panelId} role="group" aria-label={panelLabel} data-testid={panelTestid}>
      {@render panel()}
      <div class="fs-foot">
        {#if count > 0}
          <button type="button" class="btn btn--quiet" data-testid={clearTestid} onclick={onclearall}>Clear all</button>
        {/if}
        <span class="spacer"></span>
        <button type="button" class="btn btn--quiet is-bounded" data-testid={doneTestid} onclick={close}>Done</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .filters-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .fs-head {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .fs-row {
    display: flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
  }
  .search {
    flex: none;
    min-width: 0;
    height: var(--control-h-lg);
    font-size: var(--control-font);
    padding: 0 8px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .search::placeholder {
    color: var(--fg-muted);
  }
  .filters-btn {
    height: var(--control-h-lg);
    gap: 4px;
  }
  .filters-btn.has-active {
    border-color: var(--accent);
    color: var(--control-fg);
  }
  .badge {
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: var(--radius-pill);
    background: var(--accent);
    color: var(--accent-fg);
    font-size: var(--control-font-sm);
    line-height: 16px;
    font-weight: 600;
  }
  .group-wrap {
    flex: 0 1 auto;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: var(--control-h-lg);
    padding-left: 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    font-size: var(--control-font);
    color: var(--fg-muted);
  }
  .group {
    min-width: 0;
    max-width: 7.5rem;
    height: 100%;
    font-size: var(--control-font);
    padding: 0 2px;
    border: 0;
    background: transparent;
    color: var(--fg);
  }
  .fs-panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-height: 50vh;
    overflow-y: auto;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
  }
  /* The owner's sections: one heading each, a rule between them. */
  .fs-panel :global(> section) {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .fs-panel :global(> section h3) {
    margin: 0;
    font-size: var(--control-font-sm);
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
  }
  .fs-panel :global(> section + section) {
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .fs-foot {
    display: flex;
    align-items: center;
    gap: 4px;
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }
  .spacer {
    flex: 1;
  }
</style>
