<!-- The head of the left list (manual: ListFilters): pane title and count
     sentence, the search field, the Filters button with its count, each
     active filter as a removable chip, and the grouping control. It stays
     on every list screen; nothing folds into a hidden menu. -->
<script lang="ts">
  import Button from './Button.svelte';
  import Count from './Count.svelte';
  import Icon from './Icon.svelte';

  let {
    title,
    countText,
    query = $bindable(''),
    placeholder = 'Search',
    searchLabel = 'Search',
    filters = [],
    onfilters,
    onremove,
    grouping,
    ongroup,
    testid,
  }: {
    title: string;
    /** "4 need you" */
    countText?: string;
    query?: string;
    placeholder?: string;
    searchLabel?: string;
    filters?: { id: string; label: string }[];
    onfilters?: () => void;
    onremove?: (id: string) => void;
    /** The current grouping's name ("state", "host", "project"). */
    grouping?: string;
    ongroup?: () => void;
    testid?: string;
  } = $props();
</script>

<div class="of of-list list-head" data-testid={testid}>
  <div class="head">
    <span class="pane-title">{title}</span>
    {#if countText}<span class="meta">{countText}</span>{/if}
  </div>
  <label class="of-search">
    <Icon name="search" size={14} />
    <input aria-label={searchLabel} {placeholder} bind:value={query} />
  </label>
  <div class="of-filters">
    <Button onclick={onfilters}><Icon name="filter" size={12} />Filters{#if filters.length}{' '}<Count n={filters.length} />{/if}</Button>
    {#each filters as f (f.id)}
      <span class="of-chip"
        >{f.label}
        <button class="remove" aria-label="Remove filter {f.label}" onclick={() => onremove?.(f.id)}>×</button></span
      >
    {/each}
    <span class="grow"></span>
    {#if grouping}<Button variant="quiet" onclick={ongroup}>Group: {grouping} ▾</Button>{/if}
  </div>
</div>

<style>
  .list-head {
    padding: var(--space-3) var(--space-3) var(--space-2);
    gap: var(--control-gap);
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  .pane-title {
    font-size: var(--text-lg);
    line-height: var(--text-lg-lh);
    font-weight: var(--text-lg-weight);
  }
  .grow {
    flex: 1 1 auto;
  }
  .remove {
    border: 0;
    padding: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
</style>
