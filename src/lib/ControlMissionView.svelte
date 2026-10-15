<!--
  Control's Missions view (Thread room, 2026-10-14): a mission thread in the
  left column, a "Sent to a mission" chip or a task graph card's "Open full
  graph" opens that mission here, with its state, progress and the full
  task graph (MissionGraph, read-only as it is in Work); a live session on
  it opens in the Session tab. With no mission chosen it lists the
  missions. The mission itself is edited in Work ("Open in Work ↗").
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import MissionGraph from './MissionGraph.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { getMission, listMissions, openMission, progressLabel, stateLabel, type Mission, type MissionDetail } from './missions';
  import { defaultLaneBy, type LaneBy } from './mission_graph';
  import { workState } from './handoffs';
  import { onWorkChangedDebounced } from './work';
  import { controlMission, openInControl } from './control_room';
  import { showTaskInWorkView } from './work_view';

  let detail = $state<MissionDetail | null>(null);
  let list = $state<Mission[]>([]);
  let error = $state<string | null>(null);
  let laneBy = $state<LaneBy | null>(null);
  let gen = 0;

  async function load(id: number | null) {
    const mine = ++gen;
    if (id == null) {
      detail = null;
      const r = await listMissions();
      if (mine !== gen) return;
      if (r.ok && Array.isArray(r.value)) list = r.value.filter((m) => m.state !== 'cancelled');
      error = r.ok ? null : r.error.message;
      return;
    }
    const r = await getMission(id);
    if (mine !== gen) return;
    if (r.ok && r.value?.mission) {
      detail = r.value;
      error = null;
    } else {
      detail = null;
      error = r.ok ? 'That mission is not in the fleet any more.' : r.error.message;
    }
  }

  $effect(() => {
    const id = $controlMission;
    laneBy = null;
    void load(id);
  });

  const stop = onWorkChangedDebounced(
    () => void load($controlMission),
    () => 500,
    () => 3000,
  );
  onDestroy(stop);

  const lane = $derived(laneBy ?? (detail ? defaultLaneBy(detail) : 'none'));
</script>

<div class="missions" data-testid="control-missions-view">
  {#if $controlMission != null}
    <div class="head">
      <button type="button" class="back" data-testid="control-mission-back" onclick={() => controlMission.set(null)}>‹ Missions</button>
      {#if detail}
        <div class="title-row">
          <StatusDot state={workState(detail.mission.state)} />
          <span class="name" data-testid="control-mission-name">{detail.mission.name}</span>
          <button
            type="button"
            class="link"
            data-testid="control-mission-work"
            title="Open the mission in Work, where it is edited"
            onclick={() => openMission(detail!.mission.id)}>Open in Work ↗</button
          >
        </div>
        <span class="meta">{stateLabel(detail.mission.state, detail.phase)} · {progressLabel(detail.mission)}</span>
      {/if}
    </div>
    {#if error}
      <p class="empty">{error}</p>
    {:else if detail}
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
      <p class="empty">Loading the mission…</p>
    {/if}
  {:else if list.length === 0}
    <p class="empty">{error ?? 'No missions yet. Ask Control to plan one, or create one in Work.'}</p>
  {:else}
    <ul class="list">
      {#each list as m (m.id)}
        <li>
          <button type="button" class="row" data-testid="control-mission-row" onclick={() => controlMission.set(m.id)}>
            <StatusDot state={m.waiting_on ? 'waiting' : workState(m.state)} />
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
