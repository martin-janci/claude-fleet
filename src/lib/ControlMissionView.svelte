<!--
  Control's Missions view (Thread room, 2026-10-14): a mission thread in the
  left column, a "Sent to a mission" chip or a task graph card's "Open full
  graph" opens that mission here, with its state, progress and the full
  task graph (MissionGraph, read-only as it is in Work); a live session on
  it opens in the Session tab. With no mission chosen it lists the
  missions. The mission itself is edited in its Mission room ("Open mission
  room ↗"); one that waits on the person says on what, with "Answer ›".
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import MissionGraph from './MissionGraph.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { getMission, isFinal, listMissions, openMission, progressLabel, stateLabel, WAIT_WORDS, type Mission, type MissionDetail } from './missions';
  import { defaultLaneBy, type LaneBy } from './mission_graph';
  import { workState } from './handoffs';
  import { onWorkChangedDebounced } from './work';
  import { controlMission, openInControl } from './control_room';
  import { readErrorText, showTaskInWorkView } from './work_view';
  import LoadError from './states/LoadError.svelte';
  import Skeleton from './states/Skeleton.svelte';
  import type { IpcError } from './result';

  let detail = $state<MissionDetail | null>(null);
  let list = $state<Mission[]>([]);
  // Review w17-5: what the view says is told apart — still loading (a
  // skeleton, never "No missions yet"), a first read that failed (Retry), a
  // mission that is gone, and a re-read that failed while something is on
  // screen (kept, with a one-line note).
  let listLoaded = $state(false);
  let listError = $state<IpcError | null>(null);
  let detailError = $state<IpcError | null>(null);
  let gone = $state<string | null>(null);
  let refreshError = $state<IpcError | null>(null);
  let laneBy = $state<LaneBy | null>(null);
  let gen = 0;

  async function load(id: number | null) {
    const mine = ++gen;
    if (id == null) {
      detail = null;
      const r = await listMissions();
      if (mine !== gen) return;
      if (r.ok) {
        list = Array.isArray(r.value) ? r.value.filter((m) => m.state !== 'cancelled') : [];
        listLoaded = true;
        listError = null;
        refreshError = null;
      } else if (listLoaded) {
        refreshError = r.error;
      } else {
        listError = r.error;
      }
      return;
    }
    if (detail && detail.mission.id !== id) detail = null;
    const r = await getMission(id);
    if (mine !== gen) return;
    if (r.ok && r.value?.mission) {
      detail = r.value;
      detailError = null;
      gone = null;
      refreshError = null;
    } else if (r.ok || r.error.code === 'E_NOTFOUND') {
      // The hub answers a deleted or no-longer-visible mission with
      // E_NOTFOUND, never with `ok` and no mission (review w30-1).
      detail = null;
      refreshError = null;
      detailError = null;
      gone = 'That mission is not in the fleet any more.';
    } else if (detail?.mission.id === id) {
      refreshError = r.error;
    } else {
      detailError = r.error;
    }
  }

  $effect(() => {
    const id = $controlMission;
    laneBy = null;
    detailError = null;
    gone = null;
    refreshError = null;
    void load(id);
  });

  const stop = onWorkChangedDebounced(
    () => void load($controlMission),
    () => 500,
    () => 3000,
  );
  onDestroy(stop);

  const lane = $derived(laneBy ?? (detail ? defaultLaneBy(detail) : 'none'));

  /** What the mission waits on, in words, while it is not finished. */
  function waitOf(m: Mission): string | null {
    if (!m.waiting_on || isFinal(m.state)) return null;
    return WAIT_WORDS[m.waiting_on.reason] ?? 'Waits on you';
  }
  const wait = $derived(detail ? waitOf(detail.mission) : null);
</script>

<div class="missions" data-testid="control-missions-view">
  {#if $controlMission != null}
    <div class="head">
      <button type="button" class="back" data-testid="control-mission-back" onclick={() => controlMission.set(null)}>‹ Missions</button>
      {#if detail}
        <div class="title-row">
          <StatusDot state={wait ? 'waiting' : workState(detail.mission.state)} />
          <span class="name" data-testid="control-mission-name">{detail.mission.name}</span>
          <button
            type="button"
            class="link"
            data-testid="control-mission-work"
            title="Open the mission's room, where it is edited and answered"
            onclick={() => openMission(detail!.mission.id)}>Open mission room ↗</button
          >
        </div>
        <span class="meta">{stateLabel(detail.mission.state, detail.phase)} · {progressLabel(detail.mission)}</span>
        {#if wait}
          <div class="wait" data-testid="control-mission-wait">
            <span>{wait}</span>
            <button
              type="button"
              class="btn btn--primary"
              data-testid="control-mission-answer"
              onclick={() => openMission(detail!.mission.id)}>Answer ›</button
            >
          </div>
        {/if}
      {/if}
    </div>
    {#if gone}
      <p class="empty" data-testid="control-mission-gone">{gone}</p>
    {:else if detailError}
      <LoadError title="Couldn't load the mission" error={detailError} onretry={() => void load($controlMission)} testid="control-mission-load-error" />
    {:else if detail}
      {#if refreshError}
        <p class="refresh-error" role="status" data-testid="control-missions-refresh-error">
          Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
          <button class="btn btn--quiet" type="button" data-testid="control-missions-refresh-retry" onclick={() => void load($controlMission)}>Retry</button>
        </p>
      {/if}
      <div class="graph">
        <MissionGraph
          {detail}
          laneBy={lane}
          onlanechange={(by) => (laneBy = by)}
          onopenitem={(id) => showTaskInWorkView(`item:${id}`)}
          onopensession={(s) => openInControl(s.session_id, s.tmux_name)}
        />
      </div>
    {:else}
      <Skeleton rows={4} label="Loading the mission" />
    {/if}
  {:else if listError}
    <LoadError title="Couldn't load missions" error={listError} onretry={() => void load(null)} testid="control-missions-load-error" />
  {:else if !listLoaded}
    <Skeleton rows={3} label="Loading missions" />
  {:else if list.length === 0}
    <p class="empty" data-testid="control-missions-empty">No missions yet. Ask Control to plan one, or create one in Work.</p>
  {:else}
    {#if refreshError}
      <p class="refresh-error" role="status" data-testid="control-missions-refresh-error">
        Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
        <button class="btn btn--quiet" type="button" data-testid="control-missions-refresh-retry" onclick={() => void load(null)}>Retry</button>
      </p>
    {/if}
    <ul class="list">
      {#each list as m (m.id)}
        <li>
          <button type="button" class="row" data-testid="control-mission-row" onclick={() => controlMission.set(m.id)}>
            <StatusDot state={waitOf(m) ? 'waiting' : workState(m.state)} />
            <span class="name">{m.name}</span>
            <span class="meta">{progressLabel(m)}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .missions {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .wait {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--status-waiting);
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .name {
    flex: 1 1 auto;
    min-width: 0;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta,
  .empty {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .empty {
    margin: 0;
    padding: var(--space-3);
  }
  .refresh-error {
    margin: 0;
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .back,
  .link {
    align-self: flex-start;
    border: 0;
    padding: 0;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .link {
    color: var(--accent);
    flex: none;
  }
  .graph {
    padding: var(--space-2);
    overflow: auto;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row:focus-visible,
  .back:focus-visible,
  .link:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: -2px;
  }
</style>
