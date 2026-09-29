<script lang="ts">
  // Renders one page spec (`crates/fleet-core/pages/<id>.json`) with the
  // closed catalog: layout → sections (or tabs of sections) → items. The
  // spec only says what goes where; a field's label, help, bounds and danger
  // come from the registry's descriptor, a data item's formatting from its
  // source's declared shape.
  import { tick } from 'svelte';
  import FieldRow from './FieldRow.svelte';
  import DataItem from './DataItem.svelte';
  import Disclosure from './Disclosure.svelte';
  import Tabs from './Tabs.svelte';
  import AutoTidyPreview from '../AutoTidyPreview.svelte';
  import ResourcePage from './ResourcePage.svelte';
  import ReviewApply from './ReviewApply.svelte';
  import PageActionButton from './PageActionButton.svelte';
  import AccountsUsage from './usage/AccountsUsage.svelte';
  import type { SettingProposal } from './review';
  import type { ResourceType } from './resources';
  import { hosts } from '../hosts';
  import {
    boundRef,
    evalCondition,
    filterDefaults,
    filterSummary,
    isHostFilter,
    sectionsOf,
    type FilterValues,
    type Descriptor,
    type Page,
    type PageAction,
    type Section,
    type SourceSpec,
  } from './pages';

  let {
    page,
    pages,
    descs,
    values,
    sources,
    resources = [],
    actions = [],
    focusKey = null,
    readonly = false,
    remote = false,
    reason = null,
    proposals = [],
    onnavigate,
    onopen,
  }: {
    page: Page;
    pages: Page[];
    descs: Map<string, Descriptor>;
    values: Record<string, string>;
    sources: SourceSpec[];
    resources?: ResourceType[];
    /** The page actions an `action` item names. */
    actions?: PageAction[];
    /** Why the page is read-only (a paired desktop), for a resource page. */
    reason?: string | null;
    /** A setting to bring into view and highlight (a search hit). */
    focusKey?: string | null;
    /** Show every field without editing (a hub client). */
    readonly?: boolean;
    /** A paired desktop (P6): the fields are the hub's, but data items,
     *  page actions and custom components read or run on this app's own
     *  store, so none is shown. */
    remote?: boolean;
    /** Pending settings proposals (P5): a field shows its own inline, a
     *  review_apply page lists them all. */
    proposals?: SettingProposal[];
    onnavigate: (pageId: string) => void;
    /** Open a setting on its page (a review row's name). */
    onopen?: (pageId: string, key: string) => void;
  } = $props();

  const proposalOf = $derived(new Map(proposals.map((p) => [p.key, p])));
  /** Bumped after a page action ran: every data item re-reads. */
  let dataTick = $state(0);

  const tabs = $derived((page.tabs ?? []).filter((t) => evalCondition(t.when, values)));
  let tab = $state(0);

  const visibleSections = $derived<Section[]>(
    (tabs.length ? (tabs[Math.min(tab, tabs.length - 1)]?.sections ?? []) : (page.sections ?? [])).filter(
      (s) => evalCondition(s.when, values),
    ),
  );

  const modifiedCount = $derived(
    sectionsOf(page)
      .flatMap(({ section }) => section.items)
      .filter((i) => i.type === 'field')
      .filter((i) => {
        const d = descs.get(i.key);
        return d !== undefined && d.owned_by === undefined && (values[i.key] ?? d.value) !== d.default;
      }).length,
  );

  const titleOf = (id: string) => pages.find((p) => p.id === id)?.title ?? id;
  const sourceOf = (id: string) => sources.find((s) => s.id === id);

  // A data page's filter bar: each filter sets its param on every data
  // item whose source declares it. The owner keys this view by page id, so
  // the defaults are read once per page.
  // svelte-ignore state_referenced_locally
  let filterValues = $state<FilterValues>(filterDefaults(page));
  const hostAliases = $derived($hosts.map((h) => h.alias).sort());
  const summary = $derived(filterSummary(page, filterValues));
  /** Data items read this app's store: a paired desktop shows none. */
  const showData = $derived(!readonly && !remote);
  const isData = (i: Section['items'][number]) =>
    i.type === 'stat' ||
    i.type === 'record' ||
    i.type === 'table' ||
    i.type === 'chart' ||
    i.type === 'account_usage';

  let root = $state<HTMLElement>();

  // A search hit: open the tab the setting is on, then scroll to it.
  $effect(() => {
    const key = focusKey;
    if (!key) return;
    const at = (page.tabs ?? []).findIndex((t) =>
      t.sections.some((s) => s.items.some((i) => i.type === 'field' && i.key === key)),
    );
    if (at >= 0) tab = at;
    void tick().then(() => {
      const el = root?.querySelector<HTMLElement>(`[data-setting-key="${CSS.escape(key)}"]`);
      el?.scrollIntoView?.({ block: 'center' });
      el?.closest('details')?.setAttribute('open', '');
    });
  });
</script>

<div class="page" class:cards={page.layout === 'cards'} class:data={page.layout === 'data_page'} bind:this={root} data-testid={`page-${page.id}`}>
  <header>
    <h4>{page.title}</h4>
    {#if modifiedCount > 0}<span class="tag" data-testid="page-modified-count">{modifiedCount} changed</span>{/if}
  </header>
  {#if page.intro}<p class="intro">{page.intro}</p>{/if}

  {#if page.layout === 'data_page' && !showData}
    <p class="notice" data-testid="page-data-remote">
      This page reads the store of the app that owns the fleet. On a paired desktop that is the hub: read it there.
    </p>
  {:else if (page.filters ?? []).length > 0}
    <div class="filters" data-testid="page-filters">
      {#each page.filters ?? [] as f (f.param)}
        <label>
          <span>{f.label ?? f.param}</span>
          {#if isHostFilter(f)}
            <select
              data-testid={`page-filter-${f.param}`}
              value={filterValues[f.param] ?? ''}
              onchange={(e) => (filterValues[f.param] = e.currentTarget.value || null)}>
              <option value="">All hosts</option>
              {#each hostAliases as a (a)}<option value={a}>{a}</option>{/each}
            </select>
          {:else}
            <select
              data-testid={`page-filter-${f.param}`}
              value={String(filterValues[f.param])}
              onchange={(e) => (filterValues[f.param] = Number(e.currentTarget.value))}>
              {#each f.choices ?? [] as c (c)}<option value={String(c)}>last {c} d</option>{/each}
            </select>
          {/if}
        </label>
      {/each}
    </div>
  {/if}

  {#if page.layout === 'review_apply'}
    <ReviewApply {proposals} {pages} {descs} {readonly} {onopen} />
  {/if}

  {#if page.layout === 'master_detail'}
    {@const res = resources.find((r) => r.id === page.resource)}
    {#if res}
      <ResourcePage {page} resource={res} {readonly} {reason} />
    {/if}
  {:else if tabs.length > 0}
    <Tabs tabs={tabs.map((t) => t.title)} bind:selected={tab} label={page.title} testidPrefix={`page-${page.id}-tab`} />
  {/if}

  {#if page.layout !== 'master_detail'}
  {#each visibleSections.filter((s) => showData || !s.items.every(isData)) as section (section.title)}
    {#if section.collapsible || section.advanced}
      <Disclosure title={section.title} open={!section.advanced} badge={section.advanced ? 'Advanced' : undefined} testid={`section-${section.title}`}>
        {@render sectionBody(section)}
      </Disclosure>
    {:else}
      <section class="section" data-testid={`section-${section.title}`}>
        <h5>{section.title}</h5>
        {@render sectionBody(section)}
      </section>
    {/if}
  {/each}
  {/if}
</div>

{#snippet sectionBody(section: Section)}
  {#if section.intro}<p class="intro">{section.intro}</p>{/if}
  <div class="items">
    {#each section.items as item, i (i)}
      {#if item.type === 'field'}
        {@const d = descs.get(item.key)}
        {#if d && evalCondition(item.when, values)}
          <FieldRow
            {d}
            value={values[item.key] ?? d.value}
            widget={item.widget}
            hint={item.hint}
            highlighted={focusKey === item.key}
            proposal={proposalOf.get(item.key)}
            {readonly} />
        {/if}
      {:else if item.type === 'notice'}
        <p class={`notice ${item.tone}`} role={item.tone === 'info' ? undefined : 'note'}>{item.text}</p>
      {:else if item.type === 'link'}
        <button type="button" class="link" data-testid={`page-link-${item.page}`} onclick={() => onnavigate(item.page)}
          >{item.label ?? titleOf(item.page)} →</button
        >
      {:else if item.type === 'custom'}
        {#if readonly || remote}
          <!-- Custom components call local-only commands; nothing to show. -->
        {:else if item.component === 'auto_tidy_preview'}
          <AutoTidyPreview />
        {/if}
      {:else if item.type === 'action'}
        {@const action = actions.find((a) => a.id === item.action)}
        {#if action && !readonly && !remote}<PageActionButton {action} onran={() => dataTick++} />{/if}
      {:else if item.type === 'account_usage'}
        {#if showData}<AccountsUsage view={item.view} />{/if}
      {:else if showData}
        {@const spec = sourceOf(item.source.id)}
        <DataItem
          {item}
          {spec}
          source={boundRef(item.source, spec, filterValues)}
          copyTitle={spec ? (summary ? `${spec.label}, ${summary}` : spec.label) : undefined}
          tick={dataTick} />
      {/if}
    {/each}
  </div>
{/snippet}

<style>
  .page header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  h4 {
    margin: 0;
    font-size: 1rem;
  }
  h5 {
    margin: 0 0 0.35rem;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .intro {
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0.25rem 0 0.75rem;
    line-height: 1.4;
  }
  .section {
    border-top: 1px solid var(--border);
    padding: 0.6rem 0 0.4rem;
  }
  .items {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .cards .items,
  .data .items {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 1rem;
    align-items: flex-start;
  }
  .data .items > :global(figure),
  .data .items > :global(.table-wrap) {
    flex: 1 1 100%;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin: 0 0 0.6rem;
  }
  .filters label {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .filters select {
    font: inherit;
  }
  .notice {
    font-size: 0.78rem;
    margin: 0.2rem 0;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    border-left: 3px solid var(--border);
    line-height: 1.4;
  }
  .notice.warn {
    border-left-color: var(--usage-warn);
  }
  .notice.danger {
    border-left-color: var(--usage-crit);
  }
  .link {
    background: none;
    border: none;
    padding: 0.2rem 0;
    color: var(--accent);
    font: inherit;
    font-size: 0.85rem;
    cursor: pointer;
    text-align: left;
  }
  .link:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
</style>
