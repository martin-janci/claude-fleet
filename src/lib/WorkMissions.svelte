<!--
  Work → Missions, the left column (Thread room for Work, 2026-10-15; board
  W1Missions): the sidebar's Missions tab lists the missions grouped Needs
  you / Working / Draft and paused / Done this week (and a folded Earlier),
  each row with its state, the reason it is where it is (`missionReason`),
  a progress meter and N/M. A row opens the mission's room in the right
  column (MissionRoom, the `missions` destination); "+ New" opens the New
  mission form there. Filters, the Group choice and the loop footer with
  Pause all stay here.

  Text a person or a tracker wrote (names) is rendered as text, never as
  markup.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Skeleton from './states/Skeleton.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import Meter from './kit/Meter.svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { myPersonId } from './access';
  import { orgs } from './orgs';
  import { readPref, writePref } from './prefs';
  import { onWorkChangedDebounced } from './work';
  import { goTo } from './destination';
  import { readErrorText as rawErrorText } from './work_view';
  import {
    filterMissions,
    getMission,
    loopFooter,
    missionFilterChoices,
    missionFilterCount,
    missionOpenRequest,
    missionsToExplain,
    pauseAllMissions,
    stateLabel,
    withoutConfigKeys,
    type MissionDetail,
    type MissionFilters,
  } from './missions';
  import {
    loadMissionList,
    missionList,
    missionRowLine,
    missionTone,
    openMissionRoom,
    roomGroups,
    roomMissionId,
    rowProgress,
    startNewMission,
    workMissionsOpen,
  } from './mission_room';

  const missions = $derived($missionList.missions);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  // Grouped by where each mission stands (board W1), or one flat list.
  type Grouping = 'state' | 'none';
  const isGrouping = (v: unknown): v is Grouping => v === 'state' || v === 'none';
  let grouping = $state<Grouping>(readPref<Grouping>('work.missions.group', 'state', isGrouping));
  $effect(() => writePref('work.missions.group', grouping));
  let earlierOpen = $state(false);

  // The reason a row gives needs the mission's detail, read for the
  // missions still going (`missionsToExplain`), as Control does.
  let rowDetails = $state.raw<Map<number, MissionDetail>>(new Map());
  let explainSeq = 0;
  async function explainRows() {
    const mine = ++explainSeq;
    const want = missionsToExplain(missions);
    const got = await Promise.all(want.map((m) => getMission(m.id)));
    if (mine !== explainSeq) return;
    const next = new Map<number, MissionDetail>();
    got.forEach((r, i) => {
      if (r.ok && r.value?.mission) next.set(want[i].id, r.value);
    });
    rowDetails = next;
  }
  // Each list read explains its rows again.
  let lastList: unknown = null;
  $effect(() => {
    const list = $missionList.missions;
    if (list === lastList) return;
    lastList = list;
    void explainRows();
  });

  // The Filters (G7.7): needs you, mine, organisation, repository and mode.
  let filters = $state<MissionFilters>({});
  let filtersOpen = $state(false);
  const filterCount = $derived(missionFilterCount(filters));
  const filterChoices = $derived(missionFilterChoices(missions));
  const shown = $derived(filterMissions(missions, filters, $myPersonId));
  const orgName = (id: number) => (id === 0 ? 'No organisation' : ($orgs.find((o) => o.id === id)?.name ?? `Organisation ${id}`));
  const numOrNull = (v: string) => (v === '' ? null : Number(v));

  let nowSec = $state(Math.floor(Date.now() / 1000));
  const groups = $derived(
    grouping === 'state'
      ? roomGroups(shown, rowDetails, nowSec)
      : [{ key: 'all' as const, label: '', missions: [...shown].sort((a, b) => b.updated_at - a.updated_at || b.id - a.id) }],
  );
  const footer = $derived(loopFooter([...rowDetails.values()]));
  const anyActive = $derived(missions.some((m) => m.state === 'active'));

  const saveBlocked = $derived(!!hubActionBlocked('save_mission', $hubStatus, $hubConnection));
  const changeBlocked = $derived(!!hubActionBlocked('set_mission_state', $hubStatus, $hubConnection));

  function open(id: number) {
    openMissionRoom(id);
  }

  async function pauseAll() {
    busy = true;
    notice = null;
    try {
      const r = await pauseAllMissions();
      if (!r.ok) {
        notice = withoutConfigKeys(rawErrorText(r.error));
        return;
      }
      const n = r.value.length;
      notice = n ? `Paused ${n} mission${n === 1 ? '' : 's'}.` : 'No active mission to pause.';
      await loadMissionList();
    } finally {
      busy = false;
    }
  }

  // A "Sent to a mission" chip in Control, or "Open in Work ↗", asked for
  // the Missions tab (redesign 9.3): the room shows the mission named.
  $effect(() => {
    const r = $missionOpenRequest;
    if (!r) return;
    missionOpenRequest.set(null);
    if (r.id != null) openMissionRoom(r.id);
    else goTo('missions');
  });

  let stopWatching: (() => void) | null = null;
  let clock: ReturnType<typeof setInterval> | undefined;
  onMount(() => {
    void loadMissionList();
    stopWatching = onWorkChangedDebounced(() => void loadMissionList(), () => 500, () => 3000);
    clock = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 60_000);
  });
  onDestroy(() => {
    stopWatching?.();
    clearInterval(clock);
  });
</script>

<div class="missions" data-testid="work-missions">
  <div class="bar">
    {#if missions.length > 0}
      <button
        class="btn btn--quiet filters-btn"
        type="button"
        aria-expanded={filtersOpen}
        data-testid="missions-filters"
        onclick={() => (filtersOpen = !filtersOpen)}
        >Filters{#if filterCount > 0}&nbsp;<span class="count" data-testid="missions-filters-count">{filterCount}</span>{/if}</button
      >
      <label class="group-pick muted small"
        >Group
        <select bind:value={grouping} data-testid="missions-group">
          <option value="state">State</option>
          <option value="none">None</option>
        </select></label
      >
    {/if}
    <span class="grow"></span>
    <button
      class="btn btn--quiet new"
      type="button"
      disabled={saveBlocked}
      title="New mission"
      data-testid="mission-new"
      onclick={startNewMission}>+ New</button
    >
  </div>
  {#if notice}
    <p class="notice" role="status" data-testid="missions-notice">{notice}</p>
  {/if}
  {#if filtersOpen && missions.length > 0}
    <div class="filters" role="group" aria-label="Filter missions" data-testid="missions-filters-panel">
      <label class="check"><input type="checkbox" bind:checked={filters.needsYou} data-testid="missions-filter-needs-you" /> Needs you</label>
      {#if $myPersonId != null}
        <label class="check"><input type="checkbox" bind:checked={filters.mine} data-testid="missions-filter-mine" /> Mine</label>
      {/if}
      {#if filterChoices.orgIds.length > 1}
        <label class="muted small"
          >Organisation
          <select
            data-testid="missions-filter-org"
            value={filters.orgId ?? ''}
            onchange={(e) => (filters.orgId = numOrNull((e.currentTarget as HTMLSelectElement).value))}
          >
            <option value="">Any</option>
            {#each filterChoices.orgIds as id (id)}<option value={id}>{orgName(id)}</option>{/each}
          </select></label
        >
      {/if}
      {#if filterChoices.repos.length > 0}
        <label class="muted small"
          >Repository
          <select
            data-testid="missions-filter-repo"
            value={filters.projectId ?? ''}
            onchange={(e) => (filters.projectId = numOrNull((e.currentTarget as HTMLSelectElement).value))}
          >
            <option value="">Any</option>
            {#each filterChoices.repos as r (r.project_id)}<option value={r.project_id}>{r.name}</option>{/each}
          </select></label
        >
      {/if}
      {#if filterChoices.modes.length > 1}
        <label class="muted small"
          >Mode
          <select data-testid="missions-filter-mode" bind:value={filters.mode}>
            <option value={null}>Any</option>
            {#each filterChoices.modes as mode (mode)}<option value={mode}>{mode === 'continuous' ? 'Continuous' : mode === 'finite' ? 'Finite' : mode}</option>{/each}
          </select></label
        >
      {/if}
      {#if filterCount > 0}
        <button class="btn btn--quiet" type="button" data-testid="missions-filters-clear" onclick={() => (filters = {})}>Clear</button>
      {/if}
    </div>
  {/if}

  <div class="list-scroll">
    {#if $missionList.error && !$missionList.read}
      <div class="state error" role="alert" data-testid="missions-error">
        <p>{$missionList.error}</p>
        <button class="btn" type="button" onclick={() => void loadMissionList()}>Retry</button>
      </div>
    {:else if !$missionList.loaded}
      <div data-testid="missions-loading"><Skeleton rows={3} label="Loading missions" /></div>
    {:else if missions.length === 0 && !$missionList.error}
      <p class="muted empty" data-testid="missions-empty">No missions yet. A mission is a goal with the tasks that reach it.</p>
    {:else}
      {#if $missionList.error}
        <p class="refresh-error" role="status" data-testid="missions-refresh-error">
          Couldn't refresh ({$missionList.error}) — showing what was loaded.
          <button class="btn btn--quiet" type="button" data-testid="missions-refresh-retry" onclick={() => void loadMissionList()}>Retry</button>
        </p>
      {/if}
      {#each groups as g (g.key)}
        {@const folds = g.key === 'earlier'}
        {#if g.label}
          {#if folds}
            <button
              class="sec sec--fold"
              type="button"
              aria-expanded={earlierOpen}
              data-testid="missions-group-{g.key}"
              onclick={() => (earlierOpen = !earlierOpen)}
              >{g.label} <span class="count">{g.missions.length}</span> <span aria-hidden="true">{earlierOpen ? '⌄' : '›'}</span></button
            >
          {:else}
            <h4 class="sec" data-testid="missions-group-{g.key}">{g.label} <span class="count">{g.missions.length}</span></h4>
          {/if}
        {/if}
        {#if !folds || earlierOpen}
          <ul class="list" aria-label={g.label || 'Missions'}>
            {#each g.missions as m (m.id)}
              {@const d = rowDetails.get(m.id)}
              {@const why = missionRowLine(m, d)}
              {@const progress = rowProgress(m)}
              {@const on = $workMissionsOpen && $roomMissionId === m.id}
              <li>
                <button
                  class="open"
                  class:on
                  type="button"
                  aria-current={on ? 'true' : undefined}
                  data-testid="mission-row"
                  data-state={m.state}
                  data-group={g.key}
                  onclick={() => open(m.id)}
                >
                  <span class="dot"><StatusDot state={missionTone(m, d)} size={7} /></span>
                  <span class="main">
                    <span class="row-head">
                      <span class="title">{m.name}</span>
                      {#if grouping === 'none'}<span class="badge">{stateLabel(m.state)}</span>{/if}
                    </span>
                    <span class="reason" class:waiting={g.key === 'needs'} data-testid="mission-reason">{why}</span>
                  </span>
                  {#if progress}
                    <span class="progress" data-testid="mission-progress">
                      <Meter value={progress.value} level={m.state === 'failed' ? 'crit' : 'ok'} label={progress.label} />
                      <span class="tnum">{progress.text}</span>
                    </span>
                  {/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/each}
      {#if shown.length < missions.length}
        <p class="muted small hidden-line" data-testid="missions-hidden">
          {missions.length - shown.length} hidden by filters ·
          <button class="link" type="button" data-testid="missions-hidden-show" onclick={() => (filters = {})}>Show all</button>
        </p>
      {/if}
    {/if}
  </div>
  {#if footer || anyActive}
    <div class="list-foot" data-testid="missions-footer">
      {#if footer}<span class="muted small" data-testid="missions-loop">{footer}</span>{/if}
      {#if anyActive}
        <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="missions-pause-all" onclick={() => void pauseAll()}
          >Pause all</button
        >
      {/if}
    </div>
  {/if}
</div>

<style>
  .refresh-error {
    margin: 0;
    padding: 0.2rem 0.4rem;
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .missions {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    font-size: var(--text-xs);
  }
  .bar {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
    padding: 0.4rem 0.5rem;
  }
  .grow {
    flex: 1 1 auto;
  }
  .new {
    flex: none;
  }
  .list-scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding-bottom: var(--space-2);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .open {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: calc(100% - 12px);
    margin: 0 6px;
    padding: 8px 10px;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .open:hover {
    background: var(--bg-hover);
  }
  .open.on {
    background: var(--accent-soft);
  }
  .open:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -2px;
  }
  .dot {
    flex: none;
    margin-top: 5px;
    line-height: 0;
  }
  .main {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }
  .row-head {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    min-width: 0;
  }
  .title {
    flex: 1 1 auto;
    min-width: 0;
    font-size: var(--text-sm);
    font-weight: 500;
    line-height: var(--text-sm-lh);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reason {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reason.waiting {
    color: var(--status-waiting);
  }
  .progress {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 3px;
    width: 3rem;
    margin-top: 4px;
  }
  .progress :global(.of-meter) {
    width: 100%;
  }
  .tnum {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
  }
  .badge {
    font-size: var(--text-2xs);
    padding: 0.05rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    white-space: nowrap;
  }
  .sec {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    padding: 14px 14px 6px;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-weight: 500;
    letter-spacing: 0.02em;
  }
  .sec--fold {
    font: inherit;
    font-size: var(--text-2xs);
    background: none;
    border: 0;
    cursor: pointer;
  }
  .count {
    font-size: var(--text-2xs);
    font-weight: 500;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--count-bg);
    color: var(--fg-2);
  }
  .group-pick {
    display: inline-flex;
    gap: 0.3rem;
    align-items: center;
  }
  select,
  input {
    font: inherit;
    padding: 0.15rem 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    align-items: center;
    margin: 0 0.5rem 0.4rem;
    padding: 0.3rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .filters label {
    display: inline-flex;
    gap: 0.3rem;
    align-items: center;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .hidden-line,
  .empty {
    padding: var(--space-2) 14px;
  }
  .list-foot {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .small {
    font-size: var(--text-2xs);
  }
  .notice {
    margin: 0 0.5rem;
    color: var(--fg-muted);
  }
  .state.error {
    padding: var(--space-2) 14px;
  }
  .state.error p {
    color: var(--danger);
    margin: 0 0 0.3rem;
  }
</style>
