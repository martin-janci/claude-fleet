<script lang="ts">
  // The sidebar's chrome, in four layers so a filter never looks like a
  // setting and a setting never looks like an action:
  //
  //   R0  Sessions | Work                  ☑ ⚙ ↻ ‹   (view + global actions)
  //   R1  [org] search…            [Filters ²]       (Sessions list only)
  //   R2  Needs you · Select                  ⋯      (quick filters, view options)
  //   R3  Host: gpu-box × · Last 1d ×   Clear all    (only while narrowed)
  //       the filter panel (inline, pushes the list down)
  //
  // Every filter lives in one panel with labelled groups; the strip under it
  // names each one that narrows the list, with a × and "Clear all". Display
  // options (friendly names, details, grouping) sit in the ⋯ menu as
  // switches. The Work view brings its own search and filters
  // (WorkFiltersBar, the same strip and panel shape); the Sessions list's
  // controls step aside there.
  import {
    sessions,
    showBgAgents,
    showFriendlyNames,
    showRowDetails,
    sidebarGroupBy,
    type SidebarGroupBy,
  } from './sessions';
  import SegmentedControl from './SegmentedControl.svelte';
  import { diskMeter } from './hosts_view';
  import { hosts, hostFilter, effectiveHostFilter } from './hosts';
  import { hintAnchor } from './hints';
  import { accountByUuid } from './accounts';
  import { attentionIdleMinutes } from './notify';
  import Attention from './Attention.svelte';
  import ScopeAttention from './ScopeAttention.svelte';
  import { scopes, scopeSelectorShown, scopeFilter, effectiveScope, UNASSIGNED } from './orgs';
  import { scopeChordLabel, workViewChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';
  import LinkReview from './LinkReview.svelte';
  import TidyReview from './TidyReview.svelte';
  import TrackerAttention from './TrackerAttention.svelte';
  import ActiveFilters from './ActiveFilters.svelte';
  import FilterChipGroup from './FilterChipGroup.svelte';
  import { sessionFocus, clearSessionFocus } from './session_focus';
  import { RECENCY_VALUES, type Recency } from './session_status';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { trackers } from './trackers';
  import { bumpWorkChanged } from './work';
  import { sidebarView, type SidebarView } from './work_view';
  import {
    activeWorkFilterCount,
    effectiveWorkFilters,
    workFilters,
    DEFAULT_WORK_FILTERS,
    HAS_SESSION_FILTERS,
    HAS_SESSION_LABELS,
    STATUS_FILTERS,
    STATUS_FILTER_LABELS,
    statusNameFilter,
    statusNamesOf,
    type HasSessionFilter,
    type StatusCategoryFilter,
    type WorkFilters,
  } from './work_filters';
  import { clearWorkFilterPatch, sessionFacets, type SessionFacetId } from './filter_facets';

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const scopeTitle = `Organisation scope (${scopeChordLabel(isMac)})`;
  const workViewChord = workViewChordLabel(isMac);

  let {
    listView = 'sessions',
    search = $bindable(),
    recency = $bindable(),
    needsYouOnly = $bindable(),
    loading,
    loadError,
    onRefresh,
    onCollapse,
    showSettings,
    onOpenSettings,
    needsYouCount,
    selectMode,
    toggleSelectMode,
    selectedCount,
    onBulkSend,
    onBulkKill,
    onBulkCleanUp,
    onBulkArchive,
    bulkArchiveBlocked = null,
    bulkCleanUpBlocked = null,
    clearSelected,
  }: {
    /** Which list the sidebar shows under this chrome (work graph M14). In
     *  the Work view the Sessions list's own filters (search, org scope,
     *  hosts, recency, ⚑ work, Needs you, select, the focus bar) step aside —
     *  none of them narrows the Work tree, which has its own. */
    listView?: SidebarView;
    search: string;
    recency: Recency;
    needsYouOnly: boolean;
    loading: boolean;
    loadError: string | null;
    onRefresh: () => void;
    onCollapse?: () => void;
    showSettings: boolean;
    onOpenSettings: () => void;
    needsYouCount: number;
    selectMode: boolean;
    toggleSelectMode: () => void;
    selectedCount: number;
    onBulkSend: () => void;
    onBulkKill: () => void;
    /** Clean up: commit and push, then remove (redesign step 1.7). */
    onBulkCleanUp?: () => void;
    /** Archive into the work's Done, with Undo (step 1.7). */
    onBulkArchive?: () => void;
    bulkArchiveBlocked?: string | null;
    bulkCleanUpBlocked?: string | null;
    clearSelected: () => void;
  } = $props();
  const sessionsList = $derived(listView !== 'work');

  // Work keeps its own test id and toggles back to Project when pressed
  // again, as it did before it became a SegmentedControl.
  const GROUP_BY_OPTIONS: readonly { id: SidebarGroupBy; label: string; title?: string; testid?: string }[] = [
    { id: 'project', label: 'Project' },
    {
      id: 'work',
      label: 'Work',
      title: 'Group sessions by work: a ticket key (ABC-123) in a tag, branch or worktree name',
      testid: 'group-by-toggle',
    },
    // Redesign step 3.6: the flat groupings, after Project and Work.
    { id: 'state', label: 'State' },
    { id: 'host', label: 'Host' },
    { id: 'agent', label: 'Agent' },
  ];

  // ── Work filters (work graph M10.4) ──
  // Their group in the panel shows once there is work to filter (a tracker,
  // a linked session), in group-by-work, or while one is on.
  const workMode = $derived($sidebarGroupBy === 'work');
  // The tracker's own status names ("QA Review") next to the three
  // categories: whatever columns the team's workflow has, read off the
  // sessions' work, nothing to configure.
  const statusNames = $derived(statusNamesOf($sessions));
  const workView = $derived(effectiveWorkFilters($workFilters, $trackers, workMode, statusNames));
  const workActive = $derived(activeWorkFilterCount(workView));
  const workChromeShown = $derived(
    workMode || $trackers.length > 0 || workActive > 0 || $sessions.some((s) => s.work != null),
  );
  function setWork(patch: Partial<WorkFilters>) {
    workFilters.update((f) => ({ ...f, ...patch }));
  }
  const statusCategory = $derived(
    (STATUS_FILTERS as readonly string[]).includes(workView.status) ? (workView.status as StatusCategoryFilter) : null,
  );

  // ── The summary strip and the panel ──
  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));
  const scopeLabel = $derived(
    $effectiveScope === UNASSIGNED ? 'Unassigned' : ($scopes.find((s) => s.id === $effectiveScope)?.label ?? $effectiveScope),
  );
  const facets = $derived(
    sessionsList
      ? sessionFacets({
          scope: $scopeSelectorShown ? $effectiveScope : 'all',
          scopeLabel,
          host: $effectiveHostFilter,
          recency,
          search,
          needsYou: needsYouOnly,
          showBgAgents: $showBgAgents,
          work: workView,
          trackerName: (id) => $trackers.find((t) => t.id === id)?.name,
        })
      : [],
  );
  // The strip and the badge carry what the panel holds: search and Needs
  // you show their state in their own controls already. (The empty state
  // names them all.)
  const stripFacets = $derived(facets.filter((f) => f.id !== 'search' && f.id !== 'needs-you'));
  const panelCount = $derived(stripFacets.length);

  let panelOpen = $state(false);
  let filtersBtn: HTMLButtonElement | undefined = $state();
  function closePanel() {
    panelOpen = false;
    filtersBtn?.focus();
  }

  function clearFacet(id: string) {
    const fid = id as SessionFacetId;
    const patch = clearWorkFilterPatch(fid);
    if (patch) return setWork(patch);
    switch (fid) {
      case 'scope':
        return scopeFilter.set('all');
      case 'host':
        return hostFilter.set('all');
      case 'recency':
        recency = 'all';
        return;
      case 'search':
        search = '';
        return;
      case 'needs-you':
        needsYouOnly = false;
        return;
      case 'bg':
        showBgAgents.set(true);
        return;
    }
  }
  function clearAll() {
    for (const f of facets) clearFacet(f.id);
    workFilters.set({ ...DEFAULT_WORK_FILTERS });
  }

  // ── View options (⋯) ──
  let optionsOpen = $state(false);
  let optionsRoot: HTMLElement | undefined = $state();
  function onWindowPointer(e: PointerEvent) {
    if (optionsOpen && optionsRoot && !optionsRoot.contains(e.target as Node)) optionsOpen = false;
  }
  function onWindowKey(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    if (optionsOpen) optionsOpen = false;
    else if (panelOpen) closePanel();
  }

  // send_prompt / kill_session route, so they only need the live connection
  // to be up.
  const bulkSendBlocked = $derived(hubActionBlocked('send_prompt', $hubStatus, $hubConnection));
  const bulkKillBlocked = $derived(hubActionBlocked('kill_session', $hubStatus, $hubConnection));

  function refresh() {
    onRefresh();
    // The Work tree re-reads on `work:changed`.
    if (!sessionsList) bumpWorkChanged();
  }

  function accountLabel(host: { account_uuid: string | null }): string {
    if (!host.account_uuid) return '';
    const acc = $accountByUuid.get(host.account_uuid);
    if (!acc) return ` · ${host.account_uuid}`;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? ` · ${email} (${acc.seat_tier})` : ` · ${email}`;
  }
  function hostTitle(h: (typeof visibleHosts)[number]): string {
    const disk = diskMeter(h);
    return `${h.alias} — ${h.reachable ? 'reachable' : 'unreachable'}${h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}${h.claude_version ? ` · claude ${h.claude_version}` : ''}${disk ? ` · disk ${disk.text}` : ''}${accountLabel(h)}`;
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onkeydown={onWindowKey} />

<header class="sidebar-header" data-testid="sidebar-chrome-top">
  <!-- R0 — Work graph M14: two projections of one graph, Sessions (host /
       project → session → its tasks) and Work (org → group → task → its
       sessions). ⌘⇧W / Ctrl+Shift+W flips them. Global actions on the
       right: they are not filters. -->
  <div class="row r0">
    <div class="btn-group view-switch" role="tablist" aria-label="Sidebar view" data-testid="sidebar-view-switch">
      <button
        class="btn btn--chip btn--toggle"
        role="tab"
        aria-selected={$sidebarView === 'sessions'}
        class:is-active={$sidebarView === 'sessions'}
        data-testid="sidebar-view-sessions"
        title={`Sessions (${workViewChord})`}
        onclick={() => sidebarView.set('sessions')}
        >Sessions{#if !sessionsList && needsYouCount > 0}<span
            class="tab-badge hot"
            data-testid="sessions-tab-needs-you"
            title="{needsYouCount} waiting on you">{needsYouCount}</span
          >{/if}</button
      >
      <button
        class="btn btn--chip btn--toggle"
        role="tab"
        aria-selected={$sidebarView === 'work'}
        class:is-active={$sidebarView === 'work'}
        data-testid="sidebar-view-work"
        title={`Work: organisation → group → task → its sessions (${workViewChord})`}
        onclick={() => sidebarView.set('work')}>Work</button
      >
    </div>
    <span class="spacer"></span>
    <button
      class="btn btn--quiet btn--icon"
      onclick={() => onOpenSettings()}
      title="Settings"
      aria-label="Settings"
      aria-expanded={showSettings}
      data-testid="settings-open"
    >⚙</button>
    <button
      class="btn btn--quiet btn--icon"
      onclick={refresh}
      disabled={loading}
      data-testid="sidebar-refresh"
      title="Refresh"
      aria-label="Refresh"
    >{#if loading}…{:else}↻{/if}</button>
    {#if onCollapse}
      <button
        class="btn btn--quiet btn--icon"
        onclick={onCollapse}
        title="Hide sidebar (more room for terminal)"
        aria-label="Hide sidebar"
        data-testid="sidebar-collapse"
      >‹</button>
    {/if}
  </div>

  {#if sessionsList}
    <!-- R1: what to look for, and where the rest of the filters are. -->
    <div class="row">
      {#if $scopeSelectorShown}
        <!-- Work graph M5: the org scope — a view, never a boundary here. Only
             with two or more scopes, so a one-company fleet sees no chrome. -->
        <select
          class="scope"
          data-testid="scope-select"
          aria-label="Organisation scope"
          title={scopeTitle}
          value={$effectiveScope}
          onchange={(e) => scopeFilter.set((e.currentTarget as HTMLSelectElement).value)}
        >
          <option value="all">All</option>
          {#each $scopes as sc (sc.id)}
            <option value={sc.id}>{sc.label}</option>
          {/each}
          <option value={UNASSIGNED}>Unassigned</option>
        </select>
      {/if}
      <input
        class="search"
        type="search"
        placeholder="Search sessions, projects…"
        aria-label="Search sessions"
        bind:value={search}
        data-testid="sidebar-search"
      />
      <button
        bind:this={filtersBtn}
        class="btn btn--quiet is-bounded filters-btn"
        class:has-active={panelCount > 0}
        data-testid="filters-open"
        aria-expanded={panelOpen}
        aria-controls="sidebar-filter-panel"
        aria-label={panelCount > 0 ? `Filters, ${panelCount} active` : 'Filters'}
        title="Filter by machine, time, work and more"
        onclick={() => (panelOpen = !panelOpen)}
        use:hintAnchor={{ id: 'host-filter', when: visibleHosts.length >= 2 }}
      >
        <span aria-hidden="true">⏷</span> Filters{#if panelCount > 0}<span class="badge">{panelCount}</span>{/if}
      </button>
    </div>

    <!-- R2: the one-click filter and the mode that act on this list. -->
    <div class="row r2">
      <!-- One triage pill (P13/P27): the ranked queue replaces the old
           stuck-only and needs-attention pills, which ordered rows two
           different ways. -->
      <button
        class="btn btn--chip btn--toggle triage-pill"
        class:hot={needsYouCount > 0}
        data-testid="needs-you-filter"
        aria-pressed={needsYouOnly}
        title="Counts what is waiting on you now: blocked, stuck, failed, lost, safe-remove pending/failed. Toggling also shows sessions idle > {$attentionIdleMinutes} min."
        onclick={() => (needsYouOnly = !needsYouOnly)}
      >
        <!-- At zero there is nothing to warn about: the ⚠ and the count were
             permanent chrome that read as an alert. The pill stays so the
             filter remains reachable. -->
        {#if needsYouCount > 0}<span aria-hidden="true">⚠</span> Needs you <span class="count">{needsYouCount}</span>{:else}Needs you{/if}
      </button>
      <button
        class="btn btn--chip btn--toggle"
        data-testid="select-mode"
        aria-pressed={selectMode}
        title="Select several sessions (or shift/cmd-click rows) for bulk actions"
        onclick={() => toggleSelectMode()}
      >Select</button>
      <span class="spacer"></span>
      <div class="options" bind:this={optionsRoot}>
        <button
          class="btn btn--quiet btn--icon"
          data-testid="view-options-open"
          aria-label="View options"
          aria-haspopup="true"
          aria-expanded={optionsOpen}
          title="View options: names, details, grouping"
          onclick={() => (optionsOpen = !optionsOpen)}
        >⋯</button>
        {#if optionsOpen}
          <div class="menu" role="group" aria-label="View options" data-testid="view-options">
            <span class="menu-label">Group by</span>
            <div class="group-by">
              <SegmentedControl
                label="Group by"
                testidPrefix="group-by-"
                value={$sidebarGroupBy}
                options={GROUP_BY_OPTIONS}
                onchange={(id) =>
                  id === 'work'
                    ? sidebarGroupBy.update((v) => (v === 'work' ? 'project' : 'work'))
                    : sidebarGroupBy.set(id)}
              />
            </div>
            <button
              type="button"
              class="switch-row"
              role="switch"
              aria-checked={$showFriendlyNames}
              data-testid="friendly-name-toggle"
              title="Show the agent-set friendly name instead of the raw tmux name"
              onclick={() => showFriendlyNames.update((v) => !v)}
            >
              <span>Friendly names</span><span class="switch" aria-hidden="true"></span>
            </button>
            <button
              type="button"
              class="switch-row"
              role="switch"
              aria-checked={$showRowDetails}
              data-testid="toggle-row-details"
              title="Host, worktree, elapsed and badges under each session"
              onclick={() => showRowDetails.update((v) => !v)}
            >
              <span>Row details</span><span class="switch" aria-hidden="true"></span>
            </button>
          </div>
        {/if}
      </div>
    </div>

    <ActiveFilters facets={stripFacets} onclear={clearFacet} onclearall={clearAll} emptyFocus={() => filtersBtn} />

    {#if panelOpen}
      <div
        class="panel"
        id="sidebar-filter-panel"
        role="group"
        aria-label="Filters"
        data-testid="filter-panel"
      >
        <section>
          <h3>Scope</h3>
          <nav class="hosts" aria-label="host filter">
            <FilterChipGroup
              label="Machine"
              value={$effectiveHostFilter}
              options={[
                { id: 'all', label: 'Any' },
                ...visibleHosts.map((h) => ({
                  id: h.alias,
                  label: h.alias,
                  title: hostTitle(h),
                  dot: h.reachable ? ('on' as const) : ('off' as const),
                  alert: diskMeter(h)?.level === 'crit' ? 'disk almost full' : undefined,
                })),
              ]}
              testidFor={(id) => `filter-host-${id}`}
              onchange={(id) => hostFilter.set(id)}
            />
          </nav>
        </section>
        <section use:hintAnchor={{ id: 'recency-filter', when: $sessions.length > 0 }}>
          <h3>Time</h3>
          <nav class="recency" aria-label="recency filter">
            <FilterChipGroup
              label="Last active"
              value={recency}
              options={RECENCY_VALUES.map((r) => ({ id: r, label: r === 'all' ? 'Any time' : r }))}
              testidFor={(id) => `recency-${id}`}
              onchange={(id) => (recency = id)}
            />
          </nav>
        </section>
        {#if workChromeShown}
          <section class="work-section" data-testid="work-filters-sessions" aria-label="work filters">
            <h3>Work</h3>
            {#if $trackers.length > 1 || workView.tracker !== 'all'}
              <FilterChipGroup
                label="Tracker"
                value={workView.tracker}
                options={[{ id: 'all' as const, label: 'Any' }, ...$trackers.map((t) => ({ id: t.id, label: t.name }))]}
                testidFor={(id) => (id === 'all' ? 'wf-tracker-all' : `wf-tracker-${id}`)}
                onchange={(id) => setWork({ tracker: id })}
              />
            {/if}
            <FilterChipGroup
              label="Status"
              value={statusCategory ?? ''}
              options={STATUS_FILTERS.map((st) => ({ id: st, label: STATUS_FILTER_LABELS[st] }))}
              testidFor={(id) => `wf-status-${id}`}
              onchange={(id) => setWork({ status: id as StatusCategoryFilter })}
            />
            {#if statusNames.length > 0}
              <div class="fgroup" role="group" aria-label="Tracker column" data-testid="wf-status-names">
                <span class="fgroup-label">Tracker column</span>
                <div class="chips">
                  {#each statusNames as name (name)}
                    {@const f = statusNameFilter(name)}
                    {@const on = workView.status.toLowerCase() === f.toLowerCase()}
                    <button
                      type="button"
                      class="btn btn--chip btn--toggle"
                      data-testid="wf-status-name"
                      aria-pressed={on}
                      title="Only work whose tracker status is “{name}”"
                      onclick={() => setWork({ status: on ? 'all' : f })}>{name}</button
                    >
                  {/each}
                </div>
              </div>
            {/if}
            <div class="fgroup" role="group" aria-label="Assignee">
              <span class="fgroup-label">Assignee</span>
              <div class="chips">
                <button
                  type="button"
                  class="btn btn--chip btn--toggle"
                  data-testid="wf-mine"
                  aria-pressed={workView.assignee === 'mine'}
                  title="Only work assigned to you and not done (each tracker's “mine” view)"
                  onclick={() => setWork({ assignee: workView.assignee === 'mine' ? 'all' : 'mine' })}
                  >Assigned to me</button
                >
              </div>
            </div>
            {#if workMode}
              <FilterChipGroup
                label="Session"
                value={workView.hasSession}
                options={HAS_SESSION_FILTERS.map((h) => ({ id: h, label: HAS_SESSION_LABELS[h] }))}
                testidFor={(id) => `wf-session-${id}`}
                onchange={(id) => setWork({ hasSession: id as HasSessionFilter })}
              />
            {/if}
          </section>
        {/if}
        <section>
          <h3>Include</h3>
          <button
            type="button"
            class="switch-row"
            role="switch"
            aria-checked={$showBgAgents}
            data-testid="bg-toggle"
            title="Headless background agents in the list"
            onclick={() => showBgAgents.update((v) => !v)}
          >
            <span>Background agents</span><span class="switch" aria-hidden="true"></span>
          </button>
          {#if workChromeShown}
            <button
              type="button"
              class="switch-row"
              role="switch"
              aria-checked={workView.archived}
              data-testid="wf-hide-archived"
              title="Archived sessions and past work"
              onclick={() => setWork({ archived: !workView.archived })}
            >
              <span>Archived work</span><span class="switch" aria-hidden="true"></span>
            </button>
          {/if}
        </section>
        <div class="panel-foot">
          {#if panelCount > 0}
            <button type="button" class="btn btn--quiet" data-testid="wf-clear" onclick={clearAll}>Clear all</button>
          {/if}
          <span class="spacer"></span>
          <button type="button" class="btn btn--quiet is-bounded" data-testid="filters-done" onclick={closePanel}>Done</button>
        </div>
      </div>
    {/if}
  {/if}

  <Attention />
  <ScopeAttention />
  <TrackerAttention />
  <!-- Redesign 1.2: one quiet attention line, "3 links to review · 4 to
       tidy · 1 reopened", in place of the link bar and the Tidy chips. Each
       segment opens its own sheet, which wraps onto the lines below. -->
  <div class="attention-line" data-testid="attention-line">
    <LinkReview />
    <TidyReview />
  </div>
  {#if $sessionFocus && sessionsList}
    <!-- A clicked suggestion: the tree shows only this session. -->
    <div class="focus-bar" data-testid="session-focus-bar" role="status">
      <span class="focus-label">Showing only <strong>{$sessionFocus.label}</strong></span>
      <button
        class="btn btn--chip"
        data-testid="session-focus-clear"
        title="Show all sessions again"
        onclick={clearSessionFocus}
      >✕ Show all</button>
    </div>
  {/if}

  {#if selectedCount > 0 && sessionsList}
    <div class="bulk-bar" data-testid="bulk-bar" role="toolbar" aria-label="bulk actions">
      <span class="bulk-count">{selectedCount} selected</span>
      <button
        class="btn btn--chip"
        data-testid="bulk-send"
        disabled={bulkSendBlocked !== null}
        title={bulkSendBlocked ?? ''}
        onclick={() => onBulkSend()}
      >→ Send prompt</button>
      {#if onBulkArchive}
        <button
          class="btn btn--chip"
          data-testid="bulk-archive"
          disabled={bulkArchiveBlocked !== null}
          title={bulkArchiveBlocked ?? 'Move the selected sessions into their work’s Done; they keep running'}
          onclick={() => onBulkArchive()}
        >Archive</button>
      {/if}
      {#if onBulkCleanUp}
        <button
          class="btn btn--chip"
          data-testid="bulk-cleanup"
          disabled={bulkCleanUpBlocked !== null}
          title={bulkCleanUpBlocked ?? 'Commit and push each session’s work, then remove it'}
          onclick={() => onBulkCleanUp()}
        >Clean up</button>
      {/if}
      <button
        class="btn btn--chip btn--crit"
        data-testid="bulk-kill"
        disabled={bulkKillBlocked !== null}
        title={bulkKillBlocked ?? ''}
        onclick={() => onBulkKill()}
      >× Kill</button>
      <button class="btn btn--quiet" data-testid="bulk-clear" onclick={clearSelected}>Clear</button>
    </div>
  {/if}

  {#if loadError}
    <p class="err">{loadError}</p>
  {/if}
</header>

<style>
  .attention-line {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    padding: 0 0.5rem;
  }
  /* "a · b · c": a dot before every segment after the first. The segments
     belong to LinkReview and TidyReview, hence :global. */
  .attention-line :global(.al-seg ~ .al-seg)::before {
    content: '·';
    margin-right: 0.3rem;
    color: var(--fg-muted);
    text-decoration: none;
    display: inline-block;
  }
  .sidebar-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 8px 6px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .row {
    display: flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .view-switch {
    display: flex;
  }
  .view-switch .btn {
    border-radius: 0;
  }
  .view-switch .btn:first-child {
    border-radius: var(--radius-pill) 0 0 var(--radius-pill);
  }
  .view-switch .btn:last-child {
    border-radius: 0 var(--radius-pill) var(--radius-pill) 0;
    margin-left: -1px;
  }
  .tab-badge {
    margin-left: 4px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: var(--radius-pill);
    font-size: var(--control-font-sm);
    line-height: 16px;
    background: var(--usage-crit);
    color: #fff;
  }
  .search {
    flex: 1;
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
  .scope {
    flex: 0 1 auto;
    max-width: 7.5rem;
    height: var(--control-h-lg);
    font-size: var(--control-font);
    padding: 0 4px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: var(--radius-sm);
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
  .triage-pill.hot {
    color: var(--usage-crit);
    border-color: color-mix(in srgb, var(--usage-crit) 55%, transparent);
  }
  .triage-pill.hot[aria-pressed='true'] {
    background: color-mix(in srgb, var(--usage-crit) 12%, transparent);
    border-color: var(--usage-crit);
  }
  .triage-pill .count {
    font-weight: 600;
  }

  .options {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 20;
    min-width: 200px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.18);
  }
  .menu-label,
  .fgroup-label {
    font-size: var(--control-font-sm);
    font-weight: 600;
    color: var(--fg-muted);
  }
  .group-by {
    margin-bottom: 4px;
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
  .panel section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .panel h3 {
    margin: 0;
    font-size: var(--control-font-sm);
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
  }
  .panel section + section {
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .fgroup {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .panel-foot {
    display: flex;
    align-items: center;
    gap: 4px;
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }

  .bulk-bar {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 4px 6px;
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: var(--control-font);
  }
  .bulk-count {
    flex: 1;
    color: var(--fg);
  }
  .focus-bar {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 2px 6px;
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: var(--control-font);
  }
  .focus-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .err {
    color: var(--usage-crit);
    font-size: 0.8rem;
    padding: 0.2rem 0;
    margin: 0;
  }
</style>
