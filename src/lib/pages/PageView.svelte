<script lang="ts">
  // Renders one page spec (`crates/fleet-core/pages/<id>.json`) with the
  // closed catalog: layout → sections (or tabs of sections) → items. The
  // spec only says what goes where; a field's label, help, bounds and danger
  // come from the registry's descriptor, a data item's formatting from its
  // source's declared shape.
  import { tick } from 'svelte';
  import FieldRow from './FieldRow.svelte';
  import MatrixSection from './MatrixSection.svelte';
  import DataItem from './DataItem.svelte';
  import Disclosure from './Disclosure.svelte';
  import Tabs from './Tabs.svelte';
  import AutoTidyPreview from '../AutoTidyPreview.svelte';
  import ResourcePage from './ResourcePage.svelte';
  import ReviewApply from './ReviewApply.svelte';
  import GuideReview from './GuideReview.svelte';
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
    section = null,
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
    /** Show only the section with this title (a Settings tree leaf, step
     *  7.1), headed by that title, with a link to the whole page. */
    section?: string | null;
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

  /** The one section a tree leaf shows, wherever it sits (tabs included). */
  const only = $derived(section ? sectionsOf(page).find((s) => s.section.title === section)?.section : undefined);
  const tabs = $derived(only ? [] : (page.tabs ?? []).filter((t) => evalCondition(t.when, values)));
  let tab = $state(0);

  const visibleSections = $derived<Section[]>(
    (only ? [only] : tabs.length ? (tabs[Math.min(tab, tabs.length - 1)]?.sections ?? []) : (page.sections ?? [])).filter(
      (s) => evalCondition(s.when, values),
    ),
  );

  const modifiedCount = $derived(
    (only ? [only] : sectionsOf(page).map((s) => s.section))
      .flatMap((s) => s.items)
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
  /** A live source reads through its own command, which routes to the hub
   *  (11.9b): its items show on a paired desktop too. */
  const isLive = (i: Section['items'][number]) =>
    (i.type === 'stat' || i.type === 'record' || i.type === 'table' || i.type === 'chart') &&
    sourceOf(i.source.id)?.live !== undefined;
  const isData = (i: Section['items'][number]) =>
    (i.type === 'stat' ||
      i.type === 'record' ||
      i.type === 'table' ||
      i.type === 'chart' ||
      i.type === 'account_usage') &&
    !isLive(i);

  let root = $state<HTMLElement>();

  // Layout L9 `guide`: one step (section) at a time. A step whose `when`
  // no longer holds drops out, so the count follows the answers so far.
  const shownSections = $derived(visibleSections.filter((s) => showData || !s.items.every(isData)));
  let step = $state(0);
  const stepAt = $derived(Math.min(step, Math.max(0, shownSections.length - 1)));
  const lastStep = $derived(stepAt >= shownSections.length - 1);

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
    <h4>{only ? only.title : page.title}</h4>
    {#if modifiedCount > 0}<span class="tag" data-testid="page-modified-count">{modifiedCount} changed</span>{/if}
  </header>
  {#if only}
    <button type="button" class="link" data-testid="page-whole-link" onclick={() => onnavigate(page.id)}
      >All settings in {page.title} ›</button
    >
  {:else if page.intro}<p class="intro">{page.intro}</p>{/if}

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

  {#if page.layout === 'review_apply' && page.review === 'guides'}
    <GuideReview {pages} {descs} {sources} {onnavigate} />
  {:else if page.layout === 'review_apply'}
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

  {#if page.layout === 'guide'}
    {@const current = shownSections[stepAt]}
    <ol class="steps" data-testid="guide-steps">
      {#each shownSections as s, i (s.title)}
        <li class:done={i < stepAt} class:now={i === stepAt}>
          <button type="button" data-testid={`guide-step-${i}`} aria-current={i === stepAt ? 'step' : undefined} onclick={() => (step = i)}
            >{s.title}</button
          >
        </li>
      {/each}
    </ol>
    {#if current}
      <section class="section step" data-testid={`section-${current.title}`}>
        <h5 data-testid="guide-progress">Step {stepAt + 1} of {shownSections.length}: {current.title}</h5>
        {@render sectionBody(current)}
      </section>
    {/if}
    <div class="guide-nav">
      <button type="button" class="btn" data-testid="guide-back" disabled={stepAt === 0} onclick={() => (step = stepAt - 1)}>← Back</button>
      {#if lastStep}
        <button type="button" class="btn btn--primary" data-testid="guide-done" onclick={() => onnavigate(page.parent ?? 'guides')}>Done</button>
      {:else}
        <button type="button" class="btn btn--primary" data-testid="guide-next" onclick={() => (step = stepAt + 1)}>Next →</button>
      {/if}
    </div>
  {:else if page.layout !== 'master_detail'}
  {#each shownSections as section (section.title)}
    {#if only}
      <!-- A tree leaf: the header above is this section's title, and a
           section of its own is never folded away. -->
      <section class="section only" data-testid={`section-${section.title}`}>
        {@render sectionBody(section)}
      </section>
    {:else if section.collapsible || section.advanced}
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
  {#if section.matrix}
    <MatrixSection
      descs={section.items.flatMap((i) => (i.type === 'field' ? (descs.get(i.key) ?? []) : []))}
      {values}
      {readonly} />
  {:else}
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
      {:else if showData || isLive(item)}
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
  {/if}
{/snippet}

<style>
  .page header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  h4 {
    margin: 0;
    font-size: var(--text-md);
  }
  /* UX audit S2: section headings in sentence case, not uppercase labels. */
  h5 {
    margin: 0 0 0.35rem;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--fg);
  }
  .intro {
    font-size: var(--text-2xs);
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
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .filters select {
    font: inherit;
  }
  .notice {
    font-size: var(--text-2xs);
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
  /* A link on its own line, not in a sentence: it keeps the 24 px target. */
  .link {
    min-block-size: var(--control-h);
    background: none;
    border: none;
    padding: 0.2rem 0;
    color: var(--accent);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
    text-align: left;
  }
  .link:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem 0.9rem;
    list-style: none;
    counter-reset: step;
    margin: 0 0 0.5rem;
    padding: 0;
    font-size: var(--text-2xs);
  }
  .steps li {
    counter-increment: step;
  }
  .steps button {
    background: none;
    border: none;
    padding: 0.15rem 0;
    font: inherit;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .steps button::before {
    content: counter(step) '. ';
  }
  .steps li.done button {
    color: var(--fg);
  }
  .steps li.now button {
    color: var(--accent);
    font-weight: 600;
  }
  .steps button:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .guide-nav {
    display: flex;
    justify-content: space-between;
    margin-top: 0.75rem;
  }
</style>
