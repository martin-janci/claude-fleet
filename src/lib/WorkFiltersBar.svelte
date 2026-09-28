<script lang="ts">
  import { onWorkChangedDebounced } from './work';
  // The Work view's filters and saved views (work graph M14). One filters
  // object (`WorkTreeFilters`) for the tree, the saved views and the phone:
  // org, tracker, status, mine, has, review and a search (debounced). Saved
  // views live on the hub (`work_views`), so the phone and every desktop
  // share them; a write that lost a race answers `E_CONFLICT`, which reloads
  // the list and says so.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    activeFilterCount,
    activeWorkViewId,
    conflictOf,
    conflictSentence,
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
    type WorkTreeFilters,
    type WorkTreeOrg,
    type WorkTreeTracker,
    type WorkView,
  } from './work_view';

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

  // ── saved views ──
  let views = $state<WorkView[]>([]);
  let viewsError = $state<string | null>(null);
  let notice = $state<string | null>(null);
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
    if (conflictOf(e)) {
      notice = conflictSentence(what);
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
    const r = await deleteWorkView(v.id);
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
  <div class="views">
    <select
      class="view-select"
      aria-label="Saved view"
      data-testid="work-view-select"
      value={$activeWorkViewId == null ? '' : String($activeWorkViewId)}
      onchange={(e) => apply((e.currentTarget as HTMLSelectElement).value)}
    >
      <option value="">{count > 0 ? `Custom (${count} filter${count === 1 ? '' : 's'})` : 'All work'}</option>
      {#each views as v (v.id)}
        <option value={String(v.id)}>{v.name}{v.id === $activeWorkViewId && modified ? ' *' : ''}</option>
      {/each}
    </select>
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
  </div>
  {#if naming}
    <form class="name-row" onsubmit={saveAs}>
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
    <p class="notice" role="status" data-testid="work-view-notice">{notice}</p>
  {:else if viewsError}
    <p class="notice muted" data-testid="work-views-error">Saved views: {viewsError}</p>
  {/if}

  <input
    class="search"
    type="search"
    placeholder="Search key or title…"
    aria-label="Search tasks"
    data-testid="work-search"
    value={search}
    oninput={(e) => onSearch((e.currentTarget as HTMLInputElement).value)}
  />
  <div class="selects">
    <select aria-label="Organisation" data-testid="work-filter-org" value={orgValue(f.org)} onchange={(e) => onOrg((e.currentTarget as HTMLSelectElement).value)}>
      <option value="">all orgs</option>
      {#each orgs as o (o.id)}
        <option value={String(o.id)}>{o.name}</option>
      {/each}
      <option value="none">unassigned</option>
    </select>
    <select
      aria-label="Tracker"
      data-testid="work-filter-tracker"
      value={f.tracker === undefined ? '' : String(f.tracker)}
      onchange={(e) => onTracker((e.currentTarget as HTMLSelectElement).value)}
    >
      <option value="">all trackers</option>
      {#each trackers as t (t.id)}
        <option value={String(t.id)}>{t.name}</option>
      {/each}
      <option value="local">local work</option>
      <option value="ref">bare keys</option>
    </select>
    <select
      aria-label="Status"
      data-testid="work-filter-status"
      value={f.status ?? 'any'}
      onchange={(e) => set({ status: (e.currentTarget as HTMLSelectElement).value as WorkTreeFilters['status'] })}
    >
      {#each STATUS_FILTERS as s (s)}
        <option value={s}>{STATUS_FILTER_LABELS[s]}</option>
      {/each}
    </select>
    <select
      aria-label="Sessions"
      data-testid="work-filter-has"
      value={f.has ?? 'any'}
      onchange={(e) => set({ has: (e.currentTarget as HTMLSelectElement).value as WorkTreeFilters['has'] })}
    >
      {#each HAS_FILTERS as h (h)}
        <option value={h}>{HAS_FILTER_LABELS[h]}</option>
      {/each}
    </select>
  </div>
  <div class="toggles">
    <button
      class="btn btn--chip btn--toggle"
      type="button"
      aria-pressed={!!f.mine}
      data-testid="work-filter-mine"
      title="Assigned to me in its tracker"
      onclick={() => set({ mine: !f.mine })}>mine</button
    >
    <button
      class="btn btn--chip btn--toggle"
      type="button"
      aria-pressed={!!f.review}
      data-testid="work-filter-review"
      title="Only tasks with something to review"
      onclick={() => set({ review: !f.review })}>to review</button
    >
    {#if count > 0}
      <button
        class="btn btn--quiet"
        type="button"
        data-testid="work-filter-clear"
        onclick={() => {
          cancelSearch();
          workViewFilters.set({});
          search = '';
        }}>clear</button
      >
    {/if}
  </div>
</div>

<style>
  .work-filters {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.8rem;
  }
  .views,
  .name-row,
  .toggles,
  .selects {
    display: flex;
    gap: 0.25rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .view-select {
    flex: 1 1 8rem;
    min-width: 0;
  }
  .selects select {
    flex: 1 1 6.5rem;
    min-width: 0;
  }
  select,
  input {
    font: inherit;
    padding: 0.15rem 0.3rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg);
  }
  .name-row input {
    flex: 1 1 auto;
    min-width: 0;
  }
  .notice {
    margin: 0;
    font-size: 0.75rem;
  }
  .muted {
    color: var(--fg-muted);
  }
</style>
