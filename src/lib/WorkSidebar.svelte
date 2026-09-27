<script lang="ts">
  // The Work view's sidebar (work graph M14.2, read only): filters, the
  // saved-views menu, and the tree org → group → task → session
  // occurrences, from `workTreeStore`. Clicking a task opens its detail in
  // the center pane; clicking an occurrence selects that session, and every
  // occurrence of the selected session is highlighted. Titles are tracker
  // text: rendered as text, never as markup.
  import { onMount } from 'svelte';
  import { sessions } from './sessions';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { push } from './toasts';
  import { todayOpen } from './today';
  import {
    countsLabel,
    EMPTY_FILTERS,
    occurrenceKind,
    openTaskId,
    trackerDown,
    treeOccurrences,
    workTreeStore,
    type UiFilters,
    type WorkTreeStore,
  } from './work_tree';
  import type { WorkTask, WorkTaskLink } from './work_view';

  let {
    onCollapse,
    store = workTreeStore,
  }: {
    onCollapse?: () => void;
    /** The app's one Work view; injectable for tests. */
    store?: WorkTreeStore;
  } = $props();

  // svelte-ignore state_referenced_locally
  const st = store;
  onMount(() => {
    void st.ensureLoaded();
  });

  const selId = $derived($selectedSession?.id ?? null);

  // The search box is debounced; every other filter applies at once.
  let query = $state($st.filters.query);
  let queryTimer: ReturnType<typeof setTimeout> | undefined;
  // Follow a view being applied (only when the applied query itself moves,
  // not on every tree update while the person is typing).
  const appliedQuery = $derived($st.filters.query);
  $effect(() => {
    query = appliedQuery;
  });
  function onQuery(v: string) {
    query = v;
    clearTimeout(queryTimer);
    queryTimer = setTimeout(() => set({ query: v }), 250);
  }
  function set(patch: Partial<UiFilters>) {
    st.setFilters({ ...$st.filters, ...patch });
  }
  const filtered = $derived(JSON.stringify($st.filters) !== JSON.stringify(EMPTY_FILTERS));

  let viewsOpen = $state(false);
  const currentView = $derived($st.view === 'adhoc' ? null : ($st.views.find((v) => v.id === $st.view) ?? null));

  function parseOrg(v: string): UiFilters['org'] {
    if (v === '') return null;
    if (v === 'none') return 'none';
    return Number(v);
  }
  function parseTracker(v: string): UiFilters['tracker'] {
    if (v === '') return null;
    if (v === 'local' || v === 'ref') return v;
    return Number(v);
  }

  function openTask(t: WorkTask) {
    todayOpen.set(false);
    st.selectTask(t.task_id);
  }
  function openOccurrence(t: WorkTask, l: WorkTaskLink) {
    if (l.session_id == null) {
      // A past session has no live row: its task's detail says where it ran.
      openTask(t);
      return;
    }
    const row = $sessions.find((s) => s.id === l.session_id);
    if (!row) {
      push({ kind: 'info', message: `${l.name} is not in this window's session list yet — refresh the sidebar.` });
      return;
    }
    selectSessionExplicitly(row);
  }

  const OCC_TITLE: Record<string, string> = {
    primary: 'Primary task of this session',
    secondary: 'Also linked (not the primary)',
    suggested: 'Suggested — nobody has decided yet',
    past: 'Past — the session or the link has ended',
  };
</script>

<div class="work-sidebar" data-testid="work-sidebar">
  <div class="bar">
    <input
      class="search"
      type="search"
      placeholder="Search key or title"
      aria-label="Search tasks"
      value={query}
      oninput={(e) => onQuery((e.target as HTMLInputElement).value)}
      data-testid="work-search"
    />
    <div class="views">
      <button
        type="button"
        class="icon-btn"
        aria-haspopup="menu"
        aria-expanded={viewsOpen}
        title="Saved views"
        data-testid="work-views-btn"
        onclick={() => {
          viewsOpen = !viewsOpen;
          if (viewsOpen) void st.loadViews();
        }}>{currentView ? currentView.name : 'Views'} ▾</button
      >
      {#if viewsOpen}
        <div class="menu" role="menu" data-testid="work-views-menu">
          <button
            type="button"
            role="menuitemradio"
            aria-checked={$st.view === 'adhoc'}
            class="menu-item"
            onclick={() => {
              viewsOpen = false;
              st.applyView(null);
            }}>All work</button
          >
          {#each $st.views as v (v.id)}
            <button
              type="button"
              role="menuitemradio"
              aria-checked={$st.view === v.id}
              class="menu-item"
              data-testid="work-view-item"
              onclick={() => {
                viewsOpen = false;
                st.applyView(v);
              }}>{v.name}</button
            >
          {/each}
          {#if $st.viewsError}
            <p class="muted pad">Views: {$st.viewsError}</p>
          {:else if $st.views.length === 0}
            <p class="muted pad">No saved views on this hub.</p>
          {/if}
        </div>
      {/if}
    </div>
    <button type="button" class="icon-btn" title="Reload" aria-label="Reload the Work view" onclick={() => void st.reload()}
      >↻</button
    >
    {#if onCollapse}
      <button type="button" class="icon-btn" title="Hide sidebar" aria-label="Hide sidebar" onclick={onCollapse}>‹</button>
    {/if}
  </div>

  <div class="filters" data-testid="work-filters">
    <select
      aria-label="Organisation"
      value={$st.filters.org === null ? '' : String($st.filters.org)}
      onchange={(e) => set({ org: parseOrg((e.target as HTMLSelectElement).value) })}
      data-testid="work-filter-org"
    >
      <option value="">All orgs</option>
      {#each $st.orgList as o (o.id)}<option value={String(o.id)}>{o.name}</option>{/each}
      <option value="none">Unassigned</option>
    </select>
    <select
      aria-label="Tracker"
      value={$st.filters.tracker === null ? '' : String($st.filters.tracker)}
      onchange={(e) => set({ tracker: parseTracker((e.target as HTMLSelectElement).value) })}
      data-testid="work-filter-tracker"
    >
      <option value="">All trackers</option>
      {#each $st.trackers as t (t.id)}<option value={String(t.id)}>{t.name}</option>{/each}
      <option value="local">Local tasks</option>
      <option value="ref">Bare keys</option>
    </select>
    <select
      aria-label="Status"
      value={$st.filters.status}
      onchange={(e) => set({ status: (e.target as HTMLSelectElement).value as UiFilters['status'] })}
      data-testid="work-filter-status"
    >
      <option value="any">Any status</option>
      <option value="open">Open</option>
      <option value="todo">To do</option>
      <option value="in_progress">In progress</option>
      <option value="done">Done</option>
    </select>
    <select
      aria-label="Sessions"
      value={$st.filters.has}
      onchange={(e) => set({ has: (e.target as HTMLSelectElement).value as UiFilters['has'] })}
      data-testid="work-filter-has"
    >
      <option value="any">Any sessions</option>
      <option value="active">Active</option>
      <option value="past_only">Past only</option>
      <option value="none">No session</option>
      <option value="suggested">Suggested</option>
    </select>
    <label class="check"
      ><input
        type="checkbox"
        checked={$st.filters.mine}
        onchange={(e) => set({ mine: (e.target as HTMLInputElement).checked })}
        data-testid="work-filter-mine"
      /> Mine</label
    >
    <label class="check"
      ><input
        type="checkbox"
        checked={$st.filters.review}
        onchange={(e) => set({ review: (e.target as HTMLInputElement).checked })}
        data-testid="work-filter-review"
      /> To review</label
    >
    {#if filtered}
      <button type="button" class="link-btn" onclick={() => st.setFilters({ ...EMPTY_FILTERS })} data-testid="work-filter-clear"
        >Clear</button
      >
    {/if}
  </div>

  <div class="scroller">
    {#if $st.status === 'needs_hub'}
      <p class="state" data-testid="work-needs-hub">
        The Work view needs a newer hub. Update fleet-hub to see work by organisation and group; the Sessions view
        works as before.
      </p>
    {:else if $st.status === 'error'}
      <div class="state err" data-testid="work-error">
        <p>Could not load the Work view — {$st.error}</p>
        <button type="button" class="link-btn" onclick={() => void st.reload()}>Try again</button>
      </div>
    {:else if $st.status === 'loading' || $st.status === 'idle'}
      <p class="state muted" data-testid="work-loading">Loading work…</p>
    {:else if $st.orgs.length === 0}
      <p class="state muted" data-testid="work-empty">
        {filtered ? 'No tasks match these filters.' : 'No work yet. Link a session to a ticket, or connect a tracker in Settings → Work.'}
      </p>
    {:else}
      <ul class="tree" data-testid="work-tree">
        {#each $st.orgs as org (org.key)}
          {@const orgOpen = !$st.collapsedOrgs.has(org.key)}
          <li class="org" data-testid="work-org">
            <button
              type="button"
              class="org-row"
              aria-expanded={orgOpen}
              onclick={() => st.toggleOrg(org.key)}
              style={org.color ? `--org-color: ${org.color}` : undefined}
            >
              <span class="caret" class:collapsed={!orgOpen}>▾</span>
              <span class="org-bar" class:none={!org.color}></span>
              <span class="label">{org.name}</span>
              <span class="count">{org.count}</span>
            </button>
            {#if orgOpen}
              <ul class="groups">
                {#each org.sections as sec (sec.key)}
                  {@const open = $st.expanded.has(sec.key)}
                  <li class="group" data-testid="work-section" data-key={sec.key}>
                    <button
                      type="button"
                      class="group-row"
                      aria-expanded={open}
                      title={sec.group.source}
                      onclick={() => st.toggleSection(sec.key)}
                    >
                      <span class="caret" class:collapsed={!open}>▾</span>
                      <span class="label" class:nogroup={sec.group.source === 'none'}>{sec.group.label || 'No group'}</span>
                      <span class="count">{sec.count}</span>
                    </button>
                    {#if open}
                      <ul class="tasks">
                        {#each sec.tasks as t (t.task_id)}
                          <li
                            class="task"
                            class:selected={$st.selected === t.task_id}
                            class:open={$openTaskId === t.task_id}
                            data-testid="work-task"
                            data-task={t.task_id}
                          >
                            <button type="button" class="task-row" onclick={() => openTask(t)} title={t.title}>
                              {#if t.key}<span class="key" class:unavailable={t.unavailable}>{t.key}</span>{/if}
                              <span class="title" class:unavailable={t.unavailable}>{t.title || t.key || t.task_id}</span>
                              {#if t.review}<span class="badge review" title="Something to review" data-testid="work-task-review">?</span>{/if}
                              {#if t.needs_you}<span class="badge needs" data-testid="work-task-needs">needs you</span>{/if}
                            </button>
                            <div class="meta">
                              {#if t.tracker_name}<span class="tracker">{t.tracker_name}</span>{/if}
                              {#if trackerDown(t.tracker_state)}<span class="down" data-testid="work-task-down" title="The tracker is failing; this is as of its last good sync">tracker down</span>{/if}
                              {#if t.status_name ?? t.status_category}<span class="status">{t.status_name ?? t.status_category}</span>{/if}
                              {#if t.unavailable}<span class="unavail" title={t.unavailable_reason ?? ''}>unavailable</span>{/if}
                              <span class="counts" data-testid="work-task-counts">{countsLabel(t)}</span>
                            </div>
                            {#if treeOccurrences(t).length > 0}
                              <ul class="occ">
                                {#each treeOccurrences(t) as l (l.link_id)}
                                  {@const kind = occurrenceKind(l)}
                                  <li>
                                    <button
                                      type="button"
                                      class="occ-row {kind}"
                                      class:hl={l.session_id != null && l.session_id === selId}
                                      title="{OCC_TITLE[kind]}{l.why ? ` · ${l.why}` : ''}"
                                      data-testid="work-occ"
                                      data-kind={kind}
                                      data-session={l.session_id ?? ''}
                                      onclick={() => openOccurrence(t, l)}
                                    >
                                      <span class="mark">{kind === 'primary' ? '★' : kind === 'suggested' ? '?' : kind === 'past' ? '·' : '•'}</span>
                                      <span class="occ-name">{l.name}</span>
                                      {#if l.host}<span class="occ-host">{l.host}</span>{/if}
                                      {#if kind === 'past'}
                                        <span class="occ-state">ended</span>
                                      {:else if l.needs_you}
                                        <span class="occ-state needs">needs you</span>
                                      {:else if l.claude_status && kind !== 'suggested'}
                                        <span class="occ-state">{l.claude_status}</span>
                                      {/if}
                                    </button>
                                  </li>
                                {/each}
                                {#if t.sessions_more > 0}
                                  <li class="more-occ">
                                    <button type="button" class="link-btn" onclick={() => openTask(t)}>+{t.sessions_more} more</button>
                                  </li>
                                {/if}
                              </ul>
                            {/if}
                          </li>
                        {/each}
                      </ul>
                      {#if sec.error}
                        <p class="sec-state err" data-testid="work-section-error">
                          {sec.error}
                          <button type="button" class="link-btn" onclick={() => void st.loadMore(sec.key)}>Retry</button>
                        </p>
                      {:else if sec.loading}
                        <p class="sec-state muted">Loading…</p>
                      {:else if sec.cursor === undefined}
                        <p class="sec-state muted">Loading…</p>
                      {:else if sec.tasks.length === 0}
                        <p class="sec-state muted">No tasks here any more.</p>
                      {:else if sec.cursor !== null}
                        <button
                          type="button"
                          class="load-more"
                          data-testid="work-load-more"
                          onclick={() => void st.loadMore(sec.key)}
                          >Load more ({sec.tasks.length} of {sec.count})</button
                        >
                      {/if}
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape' && viewsOpen) viewsOpen = false;
  }}
/>

<style>
  .work-sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    font-size: 0.82rem;
  }
  .bar {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    padding: 0.4rem 0.6rem 0.2rem;
  }
  .search {
    flex: 1 1 auto;
    min-width: 0;
    height: var(--control-h);
    padding: 0 var(--control-px);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font-size: var(--control-font);
  }
  .icon-btn,
  select {
    height: var(--control-h);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font-size: var(--control-font-sm);
    padding: 0 0.4rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .icon-btn:hover { background: var(--control-bg-hover); }
  .views { position: relative; }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 2px);
    z-index: 5;
    min-width: 11rem;
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 0.2rem;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.18);
  }
  .menu-item {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--fg);
    padding: 0.3rem 0.5rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--control-font);
  }
  .menu-item:hover,
  .menu-item[aria-checked='true'] { background: var(--accent-soft); }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    padding: 0.2rem 0.6rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .filters select { max-width: 8.5rem; }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    font-size: var(--control-font-sm);
    color: var(--fg-muted);
  }
  .link-btn {
    background: transparent;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: var(--control-font-sm);
    padding: 0 0.2rem;
  }
  .scroller {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
    padding: 0.3rem 0.5rem;
  }
  .state { padding: 0.6rem 0.4rem; margin: 0; }
  .muted { color: var(--fg-muted); }
  .err { color: var(--usage-crit); }
  .pad { padding: 0.3rem 0.5rem; margin: 0; font-size: var(--control-font-sm); }
  ul { list-style: none; margin: 0; padding: 0; }
  .org-row,
  .group-row,
  .task-row,
  .occ-row {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    width: 100%;
    background: transparent;
    border: none;
    color: var(--fg);
    text-align: left;
    cursor: pointer;
    padding: 0.2rem 0.3rem;
    border-radius: var(--radius-sm);
    font: inherit;
  }
  .org-row:hover,
  .group-row:hover,
  .task-row:hover,
  .occ-row:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .org-row { font-weight: 600; }
  .org-bar {
    width: 3px;
    height: 0.9rem;
    border-radius: 2px;
    background: var(--org-color, var(--border));
  }
  .org-bar.none { background: var(--border); }
  .group-row { padding-left: 0.9rem; font-weight: 500; }
  .label { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nogroup { font-style: italic; color: var(--fg-muted); }
  .count { color: var(--fg-muted); font-size: var(--control-font-sm); font-variant-numeric: tabular-nums; }
  .caret { color: var(--fg-muted); font-size: 0.65rem; width: 0.7rem; transition: transform 0.1s ease; }
  .caret.collapsed { transform: rotate(-90deg); }
  .tasks { padding-left: 1.4rem; }
  .task { border-radius: var(--radius-sm); margin: 0.1rem 0; }
  .task.selected { background: color-mix(in srgb, var(--accent) 6%, transparent); }
  .task.open { box-shadow: inset 2px 0 0 var(--accent); }
  .key { font-family: var(--mono); font-size: 0.75rem; color: var(--accent); flex: none; }
  .title { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .unavailable { text-decoration: line-through; color: var(--fg-muted); }
  .badge {
    flex: none;
    font-size: 0.68rem;
    border-radius: var(--radius-pill);
    padding: 0 0.35rem;
    border: 1px solid var(--border);
  }
  .badge.review { color: var(--accent); border-color: var(--accent); }
  .badge.needs { color: var(--usage-warn); border-color: var(--usage-warn); }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    padding: 0 0.3rem 0.1rem 0.3rem;
    font-size: 0.7rem;
    color: var(--fg-muted);
  }
  .down { color: var(--usage-crit); }
  .unavail { text-decoration: line-through; }
  .counts { margin-left: auto; font-variant-numeric: tabular-nums; }
  .occ { padding-left: 0.6rem; }
  .occ-row { font-size: 0.76rem; padding: 0.1rem 0.3rem; border: 1px solid transparent; }
  .occ-row .mark { width: 0.8rem; text-align: center; color: var(--fg-muted); flex: none; }
  .occ-row.primary .mark { color: var(--usage-warn); }
  .occ-row.suggested { border: 1px dashed var(--control-border-strong); color: var(--fg-muted); }
  .occ-row.past { opacity: 0.55; }
  .occ-row.hl { background: var(--accent-soft); border-color: var(--accent); }
  .occ-name { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .occ-host, .occ-state { flex: none; font-size: 0.68rem; color: var(--fg-muted); }
  .occ-state.needs { color: var(--usage-warn); }
  .more-occ { padding-left: 1.2rem; }
  .sec-state { margin: 0.1rem 0 0.3rem 1.6rem; font-size: 0.72rem; }
  .load-more {
    margin: 0.1rem 0 0.4rem 1.6rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    font-size: 0.72rem;
    padding: 0.1rem 0.5rem;
    cursor: pointer;
  }
  .load-more:hover { color: var(--fg); border-color: var(--accent); }
</style>
