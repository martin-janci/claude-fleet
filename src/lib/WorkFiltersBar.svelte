<script lang="ts">
  import { onWorkChangedDebounced } from './work';
  // The Work view's filters and saved views (work graph M14). One filters
  // object (`WorkTreeFilters`) for the tree, the saved views and the phone:
  // org, tracker, status, mine, has, review and a search (debounced). Saved
  // views live on the hub (`work_views`), so the phone and every desktop
  // share them; a write that lost a race answers `E_CONFLICT`, which reloads
  // the list and says so.
  //
  // Laid out like the Sessions list's chrome (SidebarFilters): search and a
  // Filters button, the two quick toggles, the strip of active filters with
  // Clear all, and one panel of labelled chip groups.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import ActiveFilters from './ActiveFilters.svelte';
  import FilterChipGroup from './FilterChipGroup.svelte';
  import { withoutWorkFacet, workFacets, type WorkFacetId } from './filter_facets';
  import {
    activeFilterCount,
    activeWorkViewId,
    conflictNotice,
    deleteWorkView,
    HAS_FILTER_LABELS,
    HAS_FILTERS,
    normalizeFilters,
    sameFilters,
    saveWorkView,
    STATUS_FILTER_LABELS,
    STATUS_FILTERS,
    workViewFilters,
    workViews,
    type ConflictNotice,
    type WorkTreeFilters,
    type WorkTreeOrg,
    type WorkTreeTracker,
    type WorkView,
  } from './work_view';
  import WorkConflictNotice from './WorkConflictNotice.svelte';

  let {
    orgs,
    trackers,
    /** The search debounce, ms; injectable for tests. */
    searchDebounceMs = 300,
  }: { orgs: WorkTreeOrg[]; trackers: WorkTreeTracker[]; searchDebounceMs?: number } = $props();

  const saveBlocked = $derived(hubActionBlocked('save_work_view', $hubStatus, $hubConnection));
  const deleteBlocked = $derived(hubActionBlocked('delete_work_view', $hubStatus, $hubConnection));

  const f = $derived($workViewFilters);
  let search = $state(get(workViewFilters).query ?? '');
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  // Typed but not yet applied: the filters' old query must not overwrite it.
  let searchPending = false;
  function onSearch(v: string) {
    search = v;
    searchPending = true;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchPending = false;
      set({ query: v });
    }, searchDebounceMs);
  }
  function cancelSearch() {
    clearTimeout(searchTimer);
    searchPending = false;
  }
  // A view applied elsewhere changes the query under the input — unless the
  // person is typing (another filter changing then still carries the old
  // query, and the debounce applies theirs).
  const offF = workViewFilters.subscribe((v) => {
    if (searchPending) return;
    if ((v.query ?? '') !== search.trim()) search = v.query ?? '';
  });

  function set(patch: Partial<WorkTreeFilters>) {
    workViewFilters.update((cur) => {
      const { group: _g, ...next } = normalizeFilters({ ...cur, ...patch });
      return next;
    });
  }

  function orgValue(v: WorkTreeFilters['org']): string {
    return v === undefined ? '' : String(v);
  }
  function onOrg(v: string) {
    set({ org: v === '' ? undefined : v === 'none' ? 'none' : Number(v) });
  }
  function onTracker(v: string) {
    set({ tracker: v === '' ? undefined : v === 'local' || v === 'ref' ? v : Number(v) });
  }

  // ── the strip and the panel ──
  const facets = $derived(
    workFacets(f, {
      orgName: (id) => orgs.find((o) => o.id === id)?.name,
      trackerName: (id) => trackers.find((t) => t.id === id)?.name,
    }),
  );
  // The strip and the badge carry what the panel holds; search and the two
  // toggles show their state on screen already.
  const stripFacets = $derived(facets.filter((x) => x.id !== 'query' && x.id !== 'mine' && x.id !== 'review'));
  const panelCount = $derived(stripFacets.length);
  let panelOpen = $state(false);
  let filtersBtn: HTMLButtonElement | undefined = $state();
  function clearFacet(id: string) {
    if (id === 'query') cancelSearch();
    const { group: _g, ...next } = withoutWorkFacet(get(workViewFilters), id as WorkFacetId);
    workViewFilters.set(next);
    if (id === 'query') search = '';
  }
  function clearAll() {
    cancelSearch();
    workViewFilters.set({});
    search = '';
  }

  // ── saved views ──
  let views = $state<WorkView[]>([]);
  let viewsError = $state<string | null>(null);
  let notice = $state<string | ConflictNotice | null>(null);
  let naming = $state(false);
  let newName = $state('');
  let busy = $state(false);

  const active = $derived(views.find((v) => v.id === $activeWorkViewId) ?? null);
  const modified = $derived(active !== null && !sameFilters(active.filters ?? {}, f));
  const count = $derived(activeFilterCount(f));

  async function loadViews() {
    const r = await workViews();
    if (r.ok) {
      views = Array.isArray(r.value) ? r.value : [];
      viewsError = null;
      // A view deleted elsewhere is no longer the active one.
      const id = get(activeWorkViewId);
      if (id != null && !views.some((v) => v.id === id)) activeWorkViewId.set(null);
    } else {
      views = [];
      viewsError = r.error.message;
    }
  }

  function apply(idStr: string) {
    notice = null;
    if (idStr === '') {
      activeWorkViewId.set(null);
      return;
    }
    const v = views.find((x) => x.id === Number(idStr));
    if (!v) return;
    activeWorkViewId.set(v.id);
    cancelSearch();
    const { group: _g, ...filters } = normalizeFilters(v.filters);
    workViewFilters.set(filters);
    search = filters.query ?? '';
  }

  async function onConflictOr(e: { code: string; message: string; details?: unknown }, what: string) {
    const c = conflictNotice(e, what);
    if (c) {
      notice = c;
      await loadViews();
    } else {
      notice = e.message;
    }
  }

  async function saveAs(e?: Event) {
    e?.preventDefault();
    const name = newName.trim();
    if (!name || busy) return;
    busy = true;
    const r = await saveWorkView({ name, filters: get(workViewFilters), expected_version: 0 });
    busy = false;
    if (!r.ok) {
      await onConflictOr(r.error, `A view named “${name}”`);
      return;
    }
    naming = false;
    newName = '';
    notice = `Saved “${r.value.name}”.`;
    await loadViews();
    activeWorkViewId.set(r.value.id);
  }

  async function updateCurrent() {
    const v = active;
    if (!v || busy) return;
    busy = true;
    const r = await saveWorkView({ id: v.id, name: v.name, filters: get(workViewFilters), expected_version: v.version });
    busy = false;
    if (!r.ok) {
      await onConflictOr(r.error, `The view “${v.name}”`);
      return;
    }
    notice = `Updated “${v.name}”.`;
    await loadViews();
  }

  async function deleteCurrent() {
    const v = active;
    if (!v || busy) return;
    busy = true;
    const r = await deleteWorkView(v.id, v.version);
    busy = false;
    if (!r.ok) {
      await onConflictOr(r.error, `The view “${v.name}”`);
      return;
    }
    activeWorkViewId.set(null);
    notice = `Deleted “${v.name}”.`;
    await loadViews();
  }

  // Debounced like every other reader: session status ticks bump this too,
  // and each re-read is a `work_views` call (to the hub when paired).
  const offChanged = onWorkChangedDebounced(() => void loadViews(), () => 500);
  onMount(() => void loadViews());
  onDestroy(() => {
    offF();
    offChanged();
    clearTimeout(searchTimer);
  });
</script>

<div class="work-filters" data-testid="work-filters">
  <div class="row views">
    <select
      class="view-select"
      aria-label="Saved view"
      data-testid="work-view-select"
      value={$activeWorkViewId == null ? '' : String($activeWorkViewId)}
      onchange={(e) => apply((e.currentTarget as HTMLSelectElement).value)}
    >
      <option value="">{count > 0 ? `Custom view (${count} filter${count === 1 ? '' : 's'})` : 'All work'}</option>
      {#each views as v (v.id)}
        <option value={String(v.id)}>{v.name}{v.id === $activeWorkViewId && modified ? ' (edited)' : ''}</option>
      {/each}
    </select>
    {#if active}
      <button
        class="btn btn--quiet"
        type="button"
        data-testid="work-view-update"
        disabled={!modified || busy || saveBlocked !== null}
        title={saveBlocked ?? (modified ? `Save the current filters into “${active.name}”` : 'The view already has these filters')}
        onclick={() => void updateCurrent()}>Update</button
      >
      <button
        class="btn btn--quiet"
        type="button"
        data-testid="work-view-delete"
        disabled={busy || deleteBlocked !== null}
        title={deleteBlocked ?? `Delete “${active.name}”`}
        onclick={() => void deleteCurrent()}>Delete</button
      >
    {/if}
    <button
      class="btn btn--quiet"
      type="button"
      data-testid="work-view-save-as"
      disabled={saveBlocked !== null}
      title={saveBlocked ?? 'Save these filters as a view (shared with your phone and other desktops)'}
      onclick={() => {
        naming = !naming;
        notice = null;
      }}>Save as…</button
    >
  </div>
  {#if naming}
    <form class="row name-row" onsubmit={saveAs}>
      <input
        type="text"
        placeholder="View name"
        aria-label="View name"
        data-testid="work-view-name"
        bind:value={newName}
        maxlength="80"
      />
      <button class="btn btn--primary" type="submit" data-testid="work-view-save" disabled={!newName.trim() || busy}>Save</button>
      <button class="btn btn--quiet" type="button" onclick={() => (naming = false)}>Cancel</button>
    </form>
  {/if}
  {#if notice}
    <p class="notice" role="status" data-testid="work-view-notice">
      {#if typeof notice === 'string'}{notice}{:else}<WorkConflictNotice notice={notice} onreload={() => void loadViews()} />{/if}
    </p>
  {:else if viewsError}
    <p class="notice muted" data-testid="work-views-error">Saved views: {viewsError}</p>
  {/if}

  <div class="row">
    <input
      class="search"
      type="search"
      placeholder="Search key or title…"
      aria-label="Search tasks"
      data-testid="work-search"
      value={search}
      oninput={(e) => onSearch((e.currentTarget as HTMLInputElement).value)}
    />
    <button
      class="btn btn--quiet is-bounded filters-btn"
      class:has-active={panelCount > 0}
      type="button"
      data-testid="work-filters-open"
      bind:this={filtersBtn}
      aria-expanded={panelOpen}
      aria-controls="work-filter-panel"
      aria-label={panelCount > 0 ? `Filters, ${panelCount} active` : 'Filters'}
      title="Filter by organisation, tracker, status and sessions"
      onclick={() => (panelOpen = !panelOpen)}
    >
      <span aria-hidden="true">⏷</span> Filters{#if panelCount > 0}<span class="badge">{panelCount}</span>{/if}
    </button>
  </div>
  <div class="row toggles">
    <button
      class="btn btn--chip btn--toggle"
      type="button"
      aria-pressed={!!f.mine}
      data-testid="work-filter-mine"
      title="Assigned to me in its tracker"
      onclick={() => set({ mine: !f.mine })}>Assigned to me</button
    >
    <button
      class="btn btn--chip btn--toggle"
      type="button"
      aria-pressed={!!f.review}
      data-testid="work-filter-review"
      title="Only tasks with something to review"
      onclick={() => set({ review: !f.review })}>To review</button
    >
  </div>

  <ActiveFilters
    facets={stripFacets}
    onclear={clearFacet}
    onclearall={clearAll}
    testid="work-active-filters"
    clearAllTestid="work-filter-clear"
    emptyFocus={() => filtersBtn}
  />

  {#if panelOpen}
    <div class="panel" id="work-filter-panel" role="group" aria-label="Work filters" data-testid="work-filter-panel">
      <section data-testid="work-filter-org">
        <FilterChipGroup
          label="Organisation"
          value={orgValue(f.org)}
          options={[
            { id: '', label: 'Any' },
            ...orgs.map((o) => ({ id: String(o.id), label: o.name })),
            { id: 'none', label: 'Unassigned' },
          ]}
          testidFor={(id) => `work-filter-org-${id === '' ? 'any' : id}`}
          onchange={onOrg}
        />
      </section>
      <section data-testid="work-filter-tracker">
        <FilterChipGroup
          label="Tracker"
          value={f.tracker === undefined ? '' : String(f.tracker)}
          options={[
            { id: '', label: 'Any' },
            ...trackers.map((t) => ({ id: String(t.id), label: t.name })),
            { id: 'local', label: 'Local work', title: 'Work named in fleet, with no tracker' },
            { id: 'ref', label: 'Bare keys', title: 'A key (ABC-123) no tracker claims' },
          ]}
          testidFor={(id) => `work-filter-tracker-${id === '' ? 'any' : id}`}
          onchange={onTracker}
        />
      </section>
      <section data-testid="work-filter-status">
        <FilterChipGroup
          label="Status"
          value={f.status ?? 'any'}
          options={STATUS_FILTERS.map((s) => ({ id: s, label: STATUS_FILTER_LABELS[s] }))}
          testidFor={(id) => `work-filter-status-${id}`}
          onchange={(id) => set({ status: id })}
        />
      </section>
      <section data-testid="work-filter-has">
        <FilterChipGroup
          label="Sessions"
          value={f.has ?? 'any'}
          options={HAS_FILTERS.map((h) => ({ id: h, label: HAS_FILTER_LABELS[h] }))}
          testidFor={(id) => `work-filter-has-${id}`}
          onchange={(id) => set({ has: id })}
        />
      </section>
      <section>
        <button
          type="button"
          class="switch-row"
          role="switch"
          aria-checked={!!f.archived}
          data-testid="work-filter-archived"
          title="Done tasks, and tasks whose sessions are all archived, with nothing running"
          onclick={() => set({ archived: !f.archived })}
        >
          <span>Archived tasks</span><span class="switch" aria-hidden="true"></span>
        </button>
      </section>
      <div class="panel-foot">
        {#if panelCount > 0}
          <button type="button" class="btn btn--quiet" data-testid="work-filter-panel-clear" onclick={clearAll}>Clear all</button>
        {/if}
        <span class="spacer"></span>
        <button type="button" class="btn btn--quiet is-bounded" data-testid="work-filters-done" onclick={() => (panelOpen = false)}
          >Done</button
        >
      </div>
    </div>
  {/if}
</div>

<style>
  .work-filters {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--control-font);
  }
  .row {
    display: flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
  }
  .toggles {
    flex-wrap: wrap;
  }
  .spacer {
    flex: 1;
  }
  .view-select {
    flex: 1 1 8rem;
    min-width: 0;
  }
  select,
  input {
    font: inherit;
    height: var(--control-h-lg);
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .search,
  .name-row input {
    flex: 1 1 auto;
    min-width: 0;
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
  .panel {
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
  .panel section + section {
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .panel-foot {
    display: flex;
    align-items: center;
    gap: 4px;
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }
  .notice {
    margin: 0;
    font-size: var(--control-font-sm);
  }
  .muted {
    color: var(--fg-muted);
  }
</style>
