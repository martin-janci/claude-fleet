<script lang="ts">
  import { changedAny, onWorkChangedDebounced } from './work';
  // The Work view's filters and saved views (work graph M14). One filters
  // object (`WorkTreeFilters`) for the tree, the saved views and the phone:
  // org, tracker, status, mine, has, review and a search (debounced). Saved
  // views live on the hub (`work_views`), so the phone and every desktop
  // share them; a write that lost a race answers `E_CONFLICT`, which reloads
  // the list and says so.
  //
  // Laid out as the board "Work · tasks with filters open": search, then
  // Filters and Group; the open panel holds organisation and status chips
  // (several at once: `orgs`, `stages`), tracker and assignee pickers, the
  // live-session switch and Clear, with saved views folded under it. A
  // filter the panel no longer offers (a saved view's column or review
  // toggle) still shows in the strip, where it can be cleared.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import ActiveFilters from './ActiveFilters.svelte';
  import Icon from './kit/Icon.svelte';
  import FilterChipGroup from './FilterChipGroup.svelte';
  import { WORK_GROUPS, type WorkGroupChoice } from './filter_schema';
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
    WORK_STAGE_LABELS,
    WORK_STAGES,
    workLayout,
    workViewFilters,
    workViews,
    type ConflictNotice,
    type WorkTreeFilters,
    type WorkTreeOrg,
    type WorkStage,
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
     *  what the Assignee picker and the column chips offer. */
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

  // Organisation chips, several at once (`orgs`); a saved view's single
  // `org` reads as one of them and folds into `orgs` on the first change.
  type OrgPick = number | 'none';
  const orgChoices = $derived([
    ...orgs.map((o) => ({ id: o.id as OrgPick, label: o.name })),
    ...(f.org === 'none' || (f.orgs ?? []).includes('none') ? [{ id: 'none' as OrgPick, label: 'Unassigned' }] : []),
  ]);
  const pickedOrgs = $derived<OrgPick[]>([...(f.orgs ?? []), ...(f.org !== undefined && !(f.orgs ?? []).includes(f.org) ? [f.org] : [])]);
  function orgOn(id: OrgPick): boolean {
    return pickedOrgs.includes(id);
  }
  function toggleOrg(id: OrgPick) {
    const next = orgOn(id) ? pickedOrgs.filter((o) => o !== id) : [...pickedOrgs, id];
    set({ org: undefined, orgs: next.length > 0 ? next : undefined });
  }
  function toggleStage(st: WorkStage) {
    const cur = f.stages ?? [];
    const next = cur.includes(st) ? cur.filter((x) => x !== st) : [...cur, st];
    set({ stages: next.length > 0 ? next : undefined });
  }
  // Assignee: anyone, me (`mine`), or one person by name.
  const ME = 'me';
  const assigneeValue = $derived(f.assignee ? '@' + f.assignee.toLowerCase() : f.mine ? ME : '');
  function onAssignee(v: string) {
    if (v === ME) set({ mine: true, assignee: undefined });
    else if (v.startsWith('@')) set({ mine: undefined, assignee: peopleChips.find((p) => p.toLowerCase() === v.slice(1)) });
    else set({ mine: undefined, assignee: undefined });
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
  // "More" opens by itself when it holds a filter that is on.
  let moreOpen = $state(false);
  const moreCount = $derived(
    [f.review, f.status_name, f.has !== undefined && f.has !== 'active' ? f.has : undefined, $activeWorkViewId ?? undefined].filter(
      (x) => x !== undefined && x !== false,
    ).length,
  );
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
    // Typed but not yet applied when the bar goes (a layout switch, the
    // view closed): apply it rather than lose it (review r07).
    if (searchPending) {
      searchPending = false;
      set({ query: search });
    }
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

{#snippet orgChips()}
  <section class="group">
    <h3 class="label">Organisation</h3>
    <div class="chips" role="group" aria-label="Organisation" data-testid="work-filter-org">
      {#each orgChoices as o (o.id)}
        {@const on = orgOn(o.id)}
        <button
          class="of-chip pick"
          class:on
          type="button"
          aria-pressed={on}
          data-testid="work-filter-org-{o.id}"
          onclick={() => toggleOrg(o.id)}>{o.label}{#if on}<span aria-hidden="true"> ✓</span>{/if}</button
        >
      {/each}
    </div>
  </section>
{/snippet}

{#snippet stageChips()}
  <section class="group">
    <h3 class="label">Status</h3>
    <div class="chips" role="group" aria-label="Status" data-testid="work-filter-status">
      {#each WORK_STAGES as st (st)}
        {@const on = (f.stages ?? []).includes(st)}
        <button
          class="of-chip pick"
          class:on
          type="button"
          aria-pressed={on}
          data-testid="work-filter-stage-{st}"
          onclick={() => toggleStage(st)}>{WORK_STAGE_LABELS[st]}{#if on}<span aria-hidden="true"> ✓</span>{/if}</button
        >
      {/each}
    </div>
  </section>
{/snippet}

{#snippet pickers()}
  <div class="pair">
    <section class="group">
      <h3 class="label"><label for="work-filter-tracker">Tracker</label></h3>
      <select
        id="work-filter-tracker"
        class="of-btn pick-select"
        data-testid="work-filter-tracker"
        value={f.tracker === undefined ? '' : String(f.tracker)}
        onchange={(e) => onTracker((e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">Any</option>
        {#each trackers as t (t.id)}<option value={String(t.id)}>{t.name}</option>{/each}
        <option value="local" title="Work named in fleet, with no tracker">Local work</option>
        <option value="ref" title="A key (ABC-123) no tracker claims">Bare keys</option>
      </select>
    </section>
    <section class="group">
      <h3 class="label"><label for="work-filter-assignee">Assignee</label></h3>
      <select
        id="work-filter-assignee"
        class="of-btn pick-select"
        data-testid="work-filter-assignee"
        value={assigneeValue}
        onchange={(e) => onAssignee((e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">Anyone</option>
        <option value={ME} title="Assigned to you in its tracker">Me</option>
        {#each peopleChips as p (p.toLowerCase())}<option value={'@' + p.toLowerCase()}>{p}</option>{/each}
      </select>
    </section>
  </div>
{/snippet}

{#snippet panelFoot()}
  <section class="foot">
    <h3 class="sr-only">Sessions</h3>
    <label class="live">
      <input
        type="checkbox"
        data-testid="work-filter-live"
        checked={f.has === 'active'}
        onchange={(e) => set({ has: (e.currentTarget as HTMLInputElement).checked ? 'active' : undefined })}
      />
      Only tasks with a live session
    </label>
    <button class="of-btn quiet" type="button" data-testid="work-filter-panel-clear" disabled={count === 0} onclick={clearAll}
      >Clear</button
    >
  </section>
{/snippet}

{#snippet moreFilters()}
  <!-- Off the board, folded: what 0.5.x offered beyond its panel. -->
  <section class="more">
    <details data-testid="work-filter-more" bind:open={moreOpen}>
      <summary><h3 class="label">More</h3>{#if moreCount > 0}<span class="of-count">{moreCount}</span>{/if}</summary>
      <div class="more-body">
        {@render viewsBlock()}
        <div class="row toggles">
          <button
            class="of-chip pick"
            class:on={!!f.review}
            type="button"
            aria-pressed={!!f.review}
            data-testid="work-filter-review"
            title="Only tasks with something to review"
            onclick={() => set({ review: !f.review })}>To review</button
          >
        </div>
        <div data-testid="work-filter-has">
          <FilterChipGroup
            label="Sessions"
            value={f.has ?? 'any'}
            options={HAS_FILTERS.map((h) => ({ id: h, label: HAS_FILTER_LABELS[h] }))}
            testidFor={(id) => `work-filter-has-${id}`}
            onchange={(id) => set({ has: id === 'any' ? undefined : id })}
          />
        </div>
        {#if columnChips.length > 0}
          <div data-testid="work-filter-column">
            <FilterChipGroup
              label="Tracker column"
              value={f.status_name?.toLowerCase() ?? ''}
              options={[{ id: '', label: 'Any' }, ...columnChips.map((c) => ({ id: c.toLowerCase(), label: c }))]}
              testidFor={(id) => (id === '' ? 'work-filter-column-any' : `work-filter-column-${id}`)}
              onchange={(id) => set({ status_name: id === '' ? undefined : columnChips.find((c) => c.toLowerCase() === id) })}
            />
          </div>
        {/if}
        <label class="live">
          <input
            type="checkbox"
            data-testid="work-filter-archived"
            checked={!!f.archived}
            disabled={listLayout}
            title={listLayout ? 'In List view, Done shows them' : 'Done tasks, and tasks whose sessions are all archived, with nothing running'}
            onchange={() => set({ archived: !f.archived })}
          />
          Show archived tasks
        </label>
      </div>
    </details>
  </section>
{/snippet}

<div class="work-filters of" data-testid="work-filters">
  <!-- Board "Work · tasks with filters open": search across the pane,
       then Filters with its count and the grouping; the panel under them
       holds organisation and status chips (several at once), tracker and
       assignee, and the live-session switch. -->
  <label class="of-search">
    <Icon name="search" size={14} />
    <input
      type="search"
      aria-label="Search tasks"
      placeholder="Search tasks or keys"
      data-testid="work-search"
      value={search}
      oninput={(e) => onSearch((e.currentTarget as HTMLInputElement).value)}
    />
  </label>
  <div class="of-filters">
    <button
      bind:this={filtersBtn}
      class="of-btn filters-btn"
      class:open={panelOpen}
      type="button"
      data-testid="work-filters-open"
      aria-expanded={panelOpen}
      aria-controls="work-filter-panel"
      aria-label={panelCount > 0 ? `Filters, ${panelCount} active` : 'Filters'}
      title="Filter by organisation, status, tracker, assignee and sessions"
      onclick={() => (panelOpen = !panelOpen)}
      ><Icon name="filter" size={12} />Filters{#if panelCount > 0}<span class="of-count">{panelCount}</span>{/if}<span
        aria-hidden="true">{panelOpen ? '▴' : '▾'}</span
      ></button
    >
    <span class="grow"></span>
    <label class="of-btn quiet group-by" title="Group by">
      <span>Group:</span>
      <select
        aria-label="Group by"
        data-testid="work-group-select"
        value={groupChoice}
        onchange={(e) => onGroup((e.currentTarget as HTMLSelectElement).value as WorkGroupChoice)}
      >
        {#each WORK_GROUPS as o (o.id)}
          <option value={o.id} title={o.title}>{o.label.toLowerCase()}</option>
        {/each}
      </select>
      <span aria-hidden="true">▾</span>
    </label>
  </div>
  {#if panelOpen}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <section
      class="panel"
      id="work-filter-panel"
      aria-label="Filters"
      data-testid="work-filter-panel"
      onkeydown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          panelOpen = false;
          filtersBtn?.focus();
        }
      }}
    >
      {@render orgChips()}
      {@render stageChips()}
      {@render pickers()}
      {@render panelFoot()}
      {@render moreFilters()}
    </section>
  {/if}
  {#if notice}
    <p class="notice" role="status" data-testid="work-view-notice">
      {#if typeof notice === 'string'}{notice}{:else}<WorkConflictNotice notice={notice} onreload={() => void loadViews()} />{/if}
    </p>
  {:else if viewsError}
    <p class="notice muted" data-testid="work-views-error">Saved views: {viewsError}</p>
  {/if}

  {#if !panelOpen}
    <ActiveFilters
      facets={stripFacets}
      onclear={clearFacet}
      onclearall={clearAll}
      testid="work-active-filters"
      clearAllTestid="work-filter-clear"
      emptyFocus={() => filtersBtn}
    />
  {/if}
</div>

<style>
  .work-filters {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--control-font);
  }
  .of-search input::-webkit-search-cancel-button {
    filter: grayscale(1);
  }
  .grow {
    flex: 1 1 auto;
  }
  .filters-btn {
    padding: 0 6px;
  }
  .filters-btn.open {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--control-border));
  }
  .group-by {
    position: relative;
    padding: 0 4px;
    gap: 4px;
  }
  .group-by select {
    field-sizing: content;
    appearance: none;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    padding: 0;
    cursor: pointer;
  }
  .group-by select:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 2px;
  }
  .panel {
    margin-top: 2px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .group {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .label {
    margin: 0 0 4px;
    font-size: var(--text-xs);
    line-height: 16px;
    font-weight: normal;
    color: var(--fg-muted);
  }
  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .pick {
    border: 0;
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
  }
  .pick:hover {
    color: var(--fg);
  }
  .pick.on {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .pick-select {
    width: 100%;
    min-width: 0;
  }
  .foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 6px;
  }
  .live {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: var(--text-xs);
    line-height: 16px;
    color: var(--fg-muted);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    margin: 0;
  }
  .more {
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .more summary {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    color: var(--fg-muted);
  }
  .more summary h3 {
    margin: 0;
  }
  .more-body {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
  }
  .toggles {
    flex-wrap: wrap;
  }
  .row {
    display: flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
  }
  .view-select {
    flex: 1 1 8rem;
    min-width: 0;
  }
  .views select,
  .name-row input {
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
