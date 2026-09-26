<script lang="ts">
  import { sessions, showBgAgents, showFriendlyNames, showRowDetails, sidebarGroupBy } from './sessions';
  import { hosts, hostFilter } from './hosts';
  import { hintAnchor } from './hints';
  import { accountByUuid } from './accounts';
  import { attentionIdleMinutes } from './notify';
  import Attention from './Attention.svelte';
  import ScopeAttention from './ScopeAttention.svelte';
  import { scopes, scopeSelectorShown, scopeFilter, effectiveScope, UNASSIGNED } from './orgs';
  import { scopeChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';

  const scopeTitle = `Organisation scope (${scopeChordLabel(detectMac(typeof navigator === 'undefined' ? undefined : navigator))})`;
  import LinkReview from './LinkReview.svelte';
  import TidyReview from './TidyReview.svelte';
  import TrackerAttention from './TrackerAttention.svelte';
  import { sessionFocus, clearSessionFocus } from './session_focus';
  import { RECENCY_VALUES, type Recency } from './session_status';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { trackers } from './trackers';
  import {
    activeWorkFilterCount,
    effectiveWorkFilters,
    workFilters,
    DEFAULT_WORK_FILTERS,
    HAS_SESSION_FILTERS,
    HAS_SESSION_LABELS,
    STATUS_FILTERS,
    STATUS_FILTER_LABELS,
    type WorkFilters,
  } from './work_filters';

  // Work filters (work graph M10.4): chips under a "⚑ work" pill, shown in
  // work mode, once there is work to filter (a tracker, a linked session),
  // or while a filter is on.
  const workMode = $derived($sidebarGroupBy === 'work');
  const workView = $derived(effectiveWorkFilters($workFilters, $trackers, workMode));
  const workActive = $derived(activeWorkFilterCount(workView));
  const workChromeShown = $derived(
    workMode || $trackers.length > 0 || workActive > 0 || $sessions.some((s) => s.work != null),
  );
  let workFiltersOpen = $state(false);
  function setWork(patch: Partial<WorkFilters>) {
    workFilters.update((f) => ({ ...f, ...patch }));
  }

  // send_prompt / kill_session route, so they only need the live connection
  // to be up.
  const bulkSendBlocked = $derived(hubActionBlocked('send_prompt', $hubStatus, $hubConnection));
  const bulkKillBlocked = $derived(hubActionBlocked('kill_session', $hubStatus, $hubConnection));

  let {
    search = $bindable(),
    recency = $bindable(),
    needsYouOnly = $bindable(),
    loading,
    loadError,
    onRefresh,
    onCollapse,
    showTasks,
    showSettings,
    onOpenTasks,
    onOpenSettings,
    needsYouCount,
    selectMode,
    toggleSelectMode,
    selectedCount,
    onBulkSend,
    onBulkKill,
    clearSelected,
  }: {
    search: string;
    recency: Recency;
    needsYouOnly: boolean;
    loading: boolean;
    loadError: string | null;
    onRefresh: () => void;
    onCollapse?: () => void;
    showTasks: boolean;
    showSettings: boolean;
    onOpenTasks: () => void;
    onOpenSettings: () => void;
    needsYouCount: number;
    selectMode: boolean;
    toggleSelectMode: () => void;
    selectedCount: number;
    onBulkSend: () => void;
    onBulkKill: () => void;
    clearSelected: () => void;
  } = $props();

  function accountLabel(host: { account_uuid: string | null }): string {
    if (!host.account_uuid) return '';
    const acc = $accountByUuid.get(host.account_uuid);
    if (!acc) return `\n${host.account_uuid}`;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `\n${email} (${acc.seat_tier})` : `\n${email}`;
  }
</script>

<header class="sidebar-header" data-testid="sidebar-chrome-top">
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
      placeholder="Search sessions, projects…"
      bind:value={search}
      data-testid="sidebar-search"
    />
    <button class="icon-btn" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh" title="Refresh">
      {#if loading}…{:else}↻{/if}
    </button>
    {#if onCollapse}
      <button
        class="icon-btn"
        onclick={onCollapse}
        title="Hide sidebar (more room for terminal)"
        aria-label="Hide sidebar"
        data-testid="sidebar-collapse"
      >‹</button>
    {/if}
  </div>

  <nav class="hosts" aria-label="host filter" use:hintAnchor={{ id: 'host-filter', when: $hosts.filter((h) => !h.hidden).length >= 2 }}>
    <button
      class="pill"
      class:active={$hostFilter === 'all'}
      onclick={() => hostFilter.set('all')}
    >all</button>
    {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
      <button
        class="pill"
        class:active={$hostFilter === h.alias}
        onclick={() => hostFilter.set(h.alias)}
        title={`${h.alias}${h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}${h.claude_version ? ` · claude ${h.claude_version}` : ''}${accountLabel(h)}`}
      >
        <span class="host-dot status-{h.reachable ? 'on' : 'off'}"></span>
        {h.alias}
      </button>
    {/each}
    <button
      class="icon-btn"
      onclick={() => onOpenTasks()}
      title="Tasks (fleet-wide)"
      aria-label="Tasks"
      aria-expanded={showTasks}
      data-testid="tasks-open"
    >☑</button>
    <button
      class="icon-btn"
      onclick={() => onOpenSettings()}
      title="Settings"
      aria-label="Settings"
      aria-expanded={showSettings}
      data-testid="settings-open"
    >⚙</button>
  </nav>

  <nav class="recency" aria-label="recency filter" use:hintAnchor={{ id: 'recency-filter', when: $sessions.length > 0 }}>
    {#each RECENCY_VALUES as opt (opt)}
      <button
        class="pill"
        class:active={recency === opt}
        onclick={() => (recency = opt)}
      >
        {opt}
      </button>
    {/each}
  </nav>

  <nav class="triage" aria-label="triage filter">
    <!-- One triage pill (P13/P27): the ranked queue replaces the old
         stuck-only and needs-attention pills, which ordered rows two
         different ways. -->
    <button
      class="pill triage-pill"
      class:active={needsYouOnly}
      class:hot={needsYouCount > 0}
      data-testid="needs-you-filter"
      aria-pressed={needsYouOnly}
      title="Counts what is waiting on you now: blocked, stuck, failed, lost, safe-remove pending/failed. Toggling also shows sessions idle > {$attentionIdleMinutes} min."
      onclick={() => (needsYouOnly = !needsYouOnly)}
    >
      <!-- At zero there is nothing to warn about: the ⚠ and the "(0)" were
           permanent chrome that read as an alert. The pill stays so the
           filter remains reachable. -->
      {#if needsYouCount > 0}⚠ Needs you ({needsYouCount}){:else}Needs you{/if}
    </button>
    <button
      class="pill"
      class:active={selectMode}
      data-testid="select-mode"
      aria-pressed={selectMode}
      title="Select several sessions (or shift/cmd-click rows) for bulk actions"
      onclick={toggleSelectMode}
    >
      ☑ select
    </button>
    {#if workChromeShown}
      <button
        class="pill"
        class:active={workActive > 0}
        data-testid="work-filters-toggle"
        aria-expanded={workFiltersOpen}
        title="Filter by tracker, status, assignee, session and archived"
        onclick={() => (workFiltersOpen = !workFiltersOpen)}
      >
        ⚑ work{workActive > 0 ? ` (${workActive})` : ''}
      </button>
    {/if}
  </nav>
  {#if workChromeShown && workFiltersOpen}
    <div class="work-filters" data-testid="work-filters" role="group" aria-label="work filters">
      {#if $trackers.length > 1}
        <nav class="chips" aria-label="tracker filter">
          <button
            class="pill"
            class:active={workView.tracker === 'all'}
            data-testid="wf-tracker-all"
            onclick={() => setWork({ tracker: 'all' })}>any tracker</button
          >
          {#each $trackers as t (t.id)}
            <button
              class="pill"
              class:active={workView.tracker === t.id}
              data-testid="wf-tracker-{t.id}"
              onclick={() => setWork({ tracker: t.id })}>{t.name}</button
            >
          {/each}
        </nav>
      {/if}
      <nav class="chips" aria-label="status filter">
        {#each STATUS_FILTERS as st (st)}
          <button
            class="pill"
            class:active={workView.status === st}
            data-testid="wf-status-{st}"
            onclick={() => setWork({ status: st })}>{STATUS_FILTER_LABELS[st]}</button
          >
        {/each}
      </nav>
      <nav class="chips" aria-label="assignee and archived filters">
        <button
          class="pill"
          class:active={workView.assignee === 'mine'}
          data-testid="wf-mine"
          aria-pressed={workView.assignee === 'mine'}
          title="Only work assigned to you and not done (each tracker's “mine” view)"
          onclick={() => setWork({ assignee: workView.assignee === 'mine' ? 'all' : 'mine' })}>mine</button
        >
        <button
          class="pill"
          class:active={!workView.archived}
          data-testid="wf-hide-archived"
          aria-pressed={!workView.archived}
          title="Hide archived sessions and past work"
          onclick={() => setWork({ archived: !workView.archived })}>hide archived</button
        >
        {#if workActive > 0}
          <button
            class="pill"
            data-testid="wf-clear"
            onclick={() => workFilters.set({ ...DEFAULT_WORK_FILTERS })}>clear</button
          >
        {/if}
      </nav>
      {#if workMode}
        <nav class="chips" aria-label="session filter">
          {#each HAS_SESSION_FILTERS as h (h)}
            <button
              class="pill"
              class:active={workView.hasSession === h}
              data-testid="wf-session-{h}"
              onclick={() => setWork({ hasSession: h })}>{HAS_SESSION_LABELS[h]}</button
            >
          {/each}
        </nav>
      {/if}
    </div>
  {/if}
  <Attention />
  <ScopeAttention />
  <TrackerAttention />
  <LinkReview />
  <TidyReview />
  {#if $sessionFocus}
    <!-- A clicked suggestion: the tree shows only this session. -->
    <div class="focus-bar" data-testid="session-focus-bar" role="status">
      <span class="focus-label">Showing only <strong>{$sessionFocus.label}</strong></span>
      <button
        class="pill"
        data-testid="session-focus-clear"
        title="Show all sessions again"
        onclick={clearSessionFocus}
      >✕ show all</button>
    </div>
  {/if}

  {#if selectedCount > 0}
    <div class="bulk-bar" data-testid="bulk-bar" role="toolbar" aria-label="bulk actions">
      <span class="bulk-count">{selectedCount} selected</span>
      <button
        class="pill"
        data-testid="bulk-send"
        disabled={bulkSendBlocked !== null}
        title={bulkSendBlocked ?? ''}
        onclick={() => onBulkSend()}
      >→ Send prompt</button>
      <button
        class="pill danger"
        data-testid="bulk-kill"
        disabled={bulkKillBlocked !== null}
        title={bulkKillBlocked ?? ''}
        onclick={() => onBulkKill()}
      >× Kill</button>
      <button class="pill" data-testid="bulk-clear" onclick={clearSelected}>clear</button>
    </div>
  {/if}

  <nav class="bg-toggle" aria-label="background agents filter">
    <button
      class="pill"
      class:active={$showBgAgents}
      data-testid="bg-toggle"
      aria-pressed={$showBgAgents}
      title={$showBgAgents ? 'Hide background agents' : 'Show background agents'}
      onclick={() => showBgAgents.update((v) => !v)}
    >
      🤖 bg {$showBgAgents ? 'on' : 'off'}
    </button>
    <button
      class="pill"
      class:active={$showFriendlyNames}
      data-testid="friendly-name-toggle"
      aria-pressed={$showFriendlyNames}
      title={$showFriendlyNames
        ? 'Show raw tmux names'
        : 'Show agent-set friendly names'}
      onclick={() => showFriendlyNames.update((v) => !v)}
    >
      🏷 friendly {$showFriendlyNames ? 'on' : 'off'}
    </button>
    <button
      class="pill"
      class:active={$showRowDetails}
      data-testid="toggle-row-details"
      aria-pressed={$showRowDetails}
      title={$showRowDetails ? 'Hide the details line under each session' : 'Show host, worktree, elapsed and badges under each session'}
      onclick={() => showRowDetails.update((v) => !v)}
    >
      ≡ details {$showRowDetails ? 'on' : 'off'}
    </button>
    <button
      class="pill"
      class:active={$sidebarGroupBy === 'work'}
      data-testid="group-by-toggle"
      aria-pressed={$sidebarGroupBy === 'work'}
      title={$sidebarGroupBy === 'work'
        ? 'Group sessions by project'
        : 'Group sessions by work: a ticket key (ABC-123) in a tag, branch or worktree name'}
      onclick={() => sidebarGroupBy.update((v) => (v === 'work' ? 'project' : 'work'))}
    >
      ⧉ by {$sidebarGroupBy}
    </button>
  </nav>

  {#if loadError}
    <p class="err">{loadError}</p>
  {/if}
</header>

<style>
  .sidebar-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem 0.6rem 0.4rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .sidebar-header .row {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }
  .search {
    flex: 1;
    font-size: 0.85rem;
    padding: 0.3rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 5px;
  }
  .search::placeholder { color: var(--fg-muted); }
  .scope {
    flex: 0 0 auto;
    max-width: 7.5rem;
    font-size: 0.8rem;
    padding: 0.25rem 0.3rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 5px;
  }

  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 5px;
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    min-width: 1.6rem;
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }

  .recency { display: flex; gap: 0.25rem; }
  .work-filters {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 5px;
  }
  .work-filters .chips { display: flex; gap: 0.25rem; flex-wrap: wrap; }
  .bg-toggle { display: flex; gap: 0.25rem; flex-wrap: wrap; }
  .triage { display: flex; gap: 0.25rem; flex-wrap: wrap; align-items: center; }
  .triage-pill.hot { color: #e64a4a; border-color: rgba(230, 74, 74, 0.5); }
  .triage-pill.active { background: rgba(230, 74, 74, 0.12); }
  .pill.danger { color: #e64a4a; }
  .pill.danger:hover { border-color: #e64a4a; }
  .bulk-bar {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--accent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: 0.75rem;
  }
  .bulk-count { flex: 1; color: var(--fg); }
  .focus-bar {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    margin: 0.2rem 0.5rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid var(--accent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: 0.75rem;
  }
  .focus-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    font-size: 0.7rem;
    padding: 0.15rem 0.55rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .pill.active { color: var(--fg); border-color: var(--accent); }

  .hosts { display: flex; flex-wrap: wrap; gap: 0.25rem; align-items: center; }
  .host-dot {
    display: inline-block;
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 50%;
    margin-right: 0.3rem;
    vertical-align: middle;
  }
  .host-dot.status-on { background: rgb(80, 200, 110); }
  .host-dot.status-off { background: rgb(220, 130, 130); }

  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }
</style>
