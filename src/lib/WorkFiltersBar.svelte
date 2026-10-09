<script lang="ts">
  import { changedAny, onWorkChangedDebounced } from './work';
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
  import FiltersSection from './FiltersSection.svelte';
  import { WORK_GROUPS, type WorkGroupChoice } from './filter_schema';
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
    workLayout,
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
    /** The Work tab's List layout: its Done section always reads archived
     *  tasks, so the Archived switch does nothing there. */
    listLayout = false,
    /** The assignees and tracker columns of the tasks loaded (step 6.2):
     *  the chips a named-person and a column filter offer. */
    people = [],
    columns = [],
  }: {
    orgs: WorkTreeOrg[];
    trackers: WorkTreeTracker[];
    searchDebounceMs?: number;
    listLayout?: boolean;
    people?: readonly string[];
    columns?: readonly string[];
  } = $props();

  // A chip for the value on now, even when no task loaded carries it.
  const withCurrent = (names: readonly string[], cur: string | undefined) =>
    cur && !names.some((n) => n.toLowerCase() === cur.toLowerCase()) ? [...names, cur] : names;
  const peopleChips = $derived(withCurrent(people, $workViewFilters.assignee));
  const columnChips = $derived(withCurrent(columns, $workViewFilters.status_name));
  // The Group control: List, or Grouped with each org's sections by
  // `group_by` (redesign step 6.2).
  const groupChoice: WorkGroupChoice = $derived($workLayout === 'list' ? 'list' : ($workViewFilters.group_by ?? 'group'));
  function onGroup(id: WorkGroupChoice) {
    if (id === 'list') {
      workLayout.set('list');
      return;
    }
    workLayout.set('grouped');
    set({ group_by: id === 'group' ? undefined : id });
  }

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
  // toggles show their state on screen already. (step
  // 3.7) the toggles are in the panel, so they join the strip.
  const stripFacets = $derived(
    facets.filter((x) => x.id !== 'query'),
  );
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

  // Debounced like every other reader, and only for what moves a saved
  // view: each re-read is a `work_views` call (to the hub when paired), and
  // session status ticks and placements bump the tick too.
  const offChanged = onWorkChangedDebounced(
    (kinds) => {
      if (changedAny(kinds, 'view', 'resync', 'local')) void loadViews();
    },
    () => 500,
  );
  onMount(() => void loadViews());
  onDestroy(() => {
    offF();
    offChanged();
    clearTimeout(searchTimer);
  });
</script>

{#snippet viewsBlock()}
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
{/snippet}

{#snippet toggleChips()}
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
{/snippet}

{#snippet orgChips()}
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
{/snippet}

{#snippet trackerChips()}
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
{/snippet}

{#snippet columnChipGroup()}
  {#if columnChips.length > 0}
    <FilterChipGroup
      label="Tracker column"
      value={f.status_name?.toLowerCase() ?? ''}
      options={[{ id: '', label: 'Any' }, ...columnChips.map((c) => ({ id: c.toLowerCase(), label: c }))]}
      testidFor={(id) => (id === '' ? 'work-filter-column-any' : `work-filter-column-${id}`)}
      onchange={(id) => set({ status_name: id === '' ? undefined : columnChips.find((c) => c.toLowerCase() === id) })}
    />
  {/if}
{/snippet}

{#snippet assigneeChipGroup()}
  {#if peopleChips.length > 0}
    <FilterChipGroup
      label="Assignee"
      value={f.assignee?.toLowerCase() ?? ''}
      options={[{ id: '', label: 'Anyone' }, ...peopleChips.map((p) => ({ id: p.toLowerCase(), label: p }))]}
      testidFor={(id) => (id === '' ? 'work-filter-assignee-any' : `work-filter-assignee-${id}`)}
      onchange={(id) => set({ assignee: id === '' ? undefined : peopleChips.find((p) => p.toLowerCase() === id) })}
    />
  {/if}
{/snippet}

{#snippet statusChips()}
        <FilterChipGroup
          label="Status"
          value={f.status ?? 'any'}
          options={STATUS_FILTERS.map((s) => ({ id: s, label: STATUS_FILTER_LABELS[s] }))}
          testidFor={(id) => `work-filter-status-${id}`}
          onchange={(id) => set({ status: id })}
        />
{/snippet}

{#snippet hasChips()}
        <FilterChipGroup
          label="Sessions"
          value={f.has ?? 'any'}
          options={HAS_FILTERS.map((h) => ({ id: h, label: HAS_FILTER_LABELS[h] }))}
          testidFor={(id) => `work-filter-has-${id}`}
          onchange={(id) => set({ has: id })}
        />
{/snippet}

{#snippet archivedSwitch()}
        <button
          type="button"
          class="switch-row"
          role="switch"
          aria-checked={!!f.archived}
          data-testid="work-filter-archived"
          disabled={listLayout}
          title={listLayout ? 'In List view, Done shows them' : 'Done tasks, and tasks whose sessions are all archived, with nothing running'}
          onclick={() => set({ archived: !f.archived })}
        >
          <span>Archived tasks</span><span class="switch" aria-hidden="true"></span>
        </button>
{/snippet}

{#snippet newPanel()}
  <section>
    <h3>Saved view</h3>
    {@render viewsBlock()}
  </section>
  <section>
    <h3>Quick</h3>
    <div class="row toggles">{@render toggleChips()}</div>
  </section>
  <section data-testid="work-filter-org">
    <h3>Scope</h3>
    {@render orgChips()}
  </section>
  <section>
    <h3>Work</h3>
    <div data-testid="work-filter-tracker">{@render trackerChips()}</div>
    <div data-testid="work-filter-status">{@render statusChips()}</div>
    <div data-testid="work-filter-column">{@render columnChipGroup()}</div>
    <div data-testid="work-filter-assignee">{@render assigneeChipGroup()}</div>
    <div data-testid="work-filter-has">{@render hasChips()}</div>
  </section>
  <section>
    <h3>Include</h3>
    {@render archivedSwitch()}
  </section>
{/snippet}

<div class="work-filters" data-testid="work-filters">
    <!-- Step 3.7: the Sessions list's Filters section, one row while
         closed; saved views and the two toggles join the panel. -->
    <FiltersSection
      {search}
      onsearch={onSearch}
      searchLabel="Search tasks"
      placeholder="Search key or title…"
      searchTestid="work-search"
      count={panelCount}
      filtersTitle="Filter by saved view, organisation, tracker, status and sessions"
      filtersTestid="work-filters-open"
      panelId="work-filter-panel"
      panelLabel="Work filters"
      panelTestid="work-filter-panel"
      clearTestid="work-filter-panel-clear"
      doneTestid="work-filters-done"
      groupValue={groupChoice}
      groupOptions={WORK_GROUPS}
      ongroup={onGroup}
      groupTestid="work-group-select"
      onclearall={clearAll}
      bind:open={panelOpen}
      bind:filtersBtn
      panel={newPanel}
    />
  {#if notice}
    <p class="notice" role="status" data-testid="work-view-notice">
      {#if typeof notice === 'string'}{notice}{:else}<WorkConflictNotice notice={notice} onreload={() => void loadViews()} />{/if}
    </p>
  {:else if viewsError}
    <p class="notice muted" data-testid="work-views-error">Saved views: {viewsError}</p>
  {/if}


  <ActiveFilters
    facets={stripFacets}
    onclear={clearFacet}
    onclearall={clearAll}
    testid="work-active-filters"
    clearAllTestid="work-filter-clear"
    emptyFocus={() => filtersBtn}
  />

</div>

<style>
  .switch-row:disabled {
    opacity: 0.5;
    cursor: default;
    background: transparent;
  }
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
  .name-row input {
    flex: 1 1 auto;
    min-width: 0;
  }
  .notice {
    margin: 0;
    font-size: var(--control-font-sm);
  }
  .muted {
    color: var(--fg-muted);
  }
</style>
