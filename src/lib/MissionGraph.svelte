<script lang="ts">
  // A mission's task graph (mission_graph.ts lays it out): lanes as rows,
  // waves as columns, each task a box tinted by its state, each dependency
  // an arrow, and the longest unfinished chain marked as the critical path.
  // Hovering or focusing a task lights up what it waits for and what waits
  // on it; choosing one shows its line below the graph.
  //
  // Read-only: every write (an edge, a hold, Start wave) stays in the list,
  // so this view adds a picture, never a second way to change the mission.
  // Text a person or a tracker wrote is rendered as text, never as markup.
  import type { MissionDetail } from './missions';
  import { attemptLine, nodeCountWord, nodeLabel, verificationLabel } from './missions';
  import StatusChip from './kit/StatusChip.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import Meter from './kit/Meter.svelte';
  import {
    G,
    LANE_BY,
    SUMMARY_STATES,
    chainOf,
    layoutMission,
    progressLine,
    toneOf,
    type LaneBy,
    type NodeBox,
  } from './mission_graph';

  interface Props {
    detail: MissionDetail;
    laneBy: LaneBy;
    repoName?: (id: number) => string | null;
    onlanechange?: (by: LaneBy) => void;
  }

  let { detail, laneBy, repoName = () => null, onlanechange }: Props = $props();

  const LANE_LABEL: Record<LaneBy, string> = { repo: 'Repo', assignee: 'Assignee', none: 'None' };

  const layout = $derived(layoutMission(detail, laneBy, repoName));
  const graphNodes = $derived(layout.nodes.map((n) => n.node));

  let hover = $state<number | null>(null);
  let selected = $state<number | null>(null);
  const lit = $derived(hover != null ? chainOf(graphNodes, hover) : null);
  const chosen = $derived(layout.nodes.find((n) => n.item_id === selected) ?? null);

  const pct = $derived(layout.progress.total ? layout.progress.done / layout.progress.total : 0);
  const summary = $derived(
    SUMMARY_STATES.filter((s) => (layout.progress.byState[s] ?? 0) > 0).map((s) => ({
      state: s,
      n: layout.progress.byState[s],
    })),
  );

  function titleOf(id: number): string {
    const b = layout.nodes.find((n) => n.item_id === id);
    if (b) return b.key ?? b.title;
    const o = (detail.graph?.outside ?? []).find((x) => x.id === id);
    return o ? (o.key ?? o.title) : `#${id}`;
  }

  function label(n: NodeBox): string {
    const crit = n.critical ? ', on the critical path' : '';
    return `${n.key ? `${n.key} ` : ''}${n.title}: ${nodeLabel(n.state)}, wave ${n.wave}${crit}`;
  }

  const dim = (id: number) => lit != null && !lit.has(id);
</script>

<div class="mgraph" data-testid="mission-graph">
  <div class="head">
    <div class="progress">
      <span class="line" data-testid="mission-graph-progress">{progressLine(layout.progress)}</span>
      <Meter value={pct} label="Tasks done" testid="mission-graph-meter" />
    </div>
    <div class="chips" data-testid="mission-graph-summary">
      {#each summary as s (s.state)}
        <span class="chip t-{s.state}"><StatusDot state={toneOf(s.state)} label={null} size={6} /> {s.n} {nodeCountWord(s.state)}</span>
      {/each}
      {#if layout.critical.length > 1}
        <span class="chip crit" data-testid="mission-graph-critical">Critical path · {layout.critical.length} tasks</span>
      {/if}
    </div>
    <label class="lanes">
      <span class="muted">Lanes</span>
      <select
        value={laneBy}
        data-testid="mission-graph-lanes"
        onchange={(e) => onlanechange?.((e.currentTarget as HTMLSelectElement).value as LaneBy)}
      >
        {#each LANE_BY as b (b)}<option value={b}>{LANE_LABEL[b]}</option>{/each}
      </select>
    </label>
  </div>

  {#if layout.nodes.length === 0}
    <p class="muted empty">No tasks yet. Add one below or ask the planner for a plan.</p>
  {:else}
    <div class="scroll">
      <div class="canvas" style:width="{layout.width}px" style:height="{layout.height}px">
        {#each layout.waves as w (w.wave)}
          <div class="wave-head" style:left="{w.x}px" style:width="{G.col}px" data-testid="mission-graph-wave">
            W{w.wave} <span class="muted">{w.done}/{w.total}</span>
          </div>
        {/each}
        {#each layout.lanes as l, i (l.id)}
          <div class="lane" class:alt={i % 2 === 1} style:top="{l.y}px" style:height="{l.h}px" data-testid="mission-graph-lane">
            <div class="lane-head" style:width="{G.laneHead}px">
              <span class="lane-name" title={l.label}>{l.label}</span>
              <span class="muted small">{l.done}/{l.total} done</span>
            </div>
          </div>
        {/each}
        <svg class="edges" width={layout.width} height={layout.height} aria-hidden="true">
          <defs>
            <marker id="mg-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto">
              <path d="M0,0 L8,4 L0,8 z" fill="context-stroke" />
            </marker>
          </defs>
          {#each layout.edges as e (`${e.from}>${e.to}`)}
            <path
              d={e.d}
              class="edge"
              class:met={e.met}
              class:crit={e.critical}
              class:dim={lit != null && !(lit.has(e.from) && lit.has(e.to))}
              marker-end="url(#mg-arrow)"
            />
          {/each}
        </svg>
        {#each layout.nodes as n (n.item_id)}
          <button
            type="button"
            class="node t-{n.tone}"
            class:crit={n.critical}
            class:dim={dim(n.item_id)}
            class:sel={selected === n.item_id}
            style:left="{n.x}px"
            style:top="{n.y}px"
            style:width="{G.nodeW}px"
            style:height="{G.node}px"
            aria-label={label(n)}
            aria-pressed={selected === n.item_id}
            data-testid="mission-graph-node"
            data-state={n.state}
            onmouseenter={() => (hover = n.item_id)}
            onmouseleave={() => (hover = null)}
            onfocus={() => (hover = n.item_id)}
            onblur={() => (hover = null)}
            onclick={() => (selected = selected === n.item_id ? null : n.item_id)}
          >
            <StatusDot state={n.tone} label={null} />
            <span class="text">
              <span class="title">{n.title}</span>
              <span class="sub">{n.key ? `${n.key} · ` : ''}{nodeLabel(n.state)}</span>
            </span>
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if chosen}
    <div class="chosen" role="status" data-testid="mission-graph-chosen">
      <StatusChip state={toneOf(chosen.state)} label={nodeLabel(chosen.state)} />
      <span class="title">{chosen.key ? `${chosen.key} · ` : ''}{chosen.title}</span>
      <span class="muted small">W{chosen.wave} · {chosen.lane}</span>
      {#if (chosen.node.waiting_for ?? []).length > 0}
        <span class="muted small">waits for {(chosen.node.waiting_for ?? []).map(titleOf).join(', ')}</span>
      {/if}
      {#if chosen.node.attempt}<span class="muted small">{attemptLine(chosen.node.attempt)}</span>{/if}
      {#if chosen.node.verification}
        <span class="muted small">{verificationLabel(chosen.node.verification.state)}</span>
      {/if}
      {#if chosen.url}<a class="small" href={chosen.url} target="_blank" rel="noreferrer">Open in tracker</a>{/if}
    </div>
  {/if}
</div>

<style>
  .mgraph { display: flex; flex-direction: column; gap: 8px; }
  .head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 16px; }
  .progress { display: flex; flex-direction: column; gap: 4px; min-width: 180px; }
  .progress .line { font-size: var(--text-sm); font-weight: 500; color: var(--fg); }
  /* The design system's Meter (of-meter). */
  .chips { display: flex; flex-wrap: wrap; gap: 6px; flex: 1 1 auto; }
  /* The design system's StatusChip (of-chip). */
  .chip {
    height: 20px; padding: 0 6px; border-radius: var(--radius-sm); font-size: var(--text-2xs); font-weight: 500;
    display: inline-flex; align-items: center; gap: 4px; background: var(--chip-bg); color: var(--fg-2); white-space: nowrap;
  }
  .chip.t-done { background: var(--done-soft); color: var(--status-done); }
  .chip.t-failed, .chip.t-blocked { background: var(--failed-soft); color: var(--status-failed); }
  .chip.t-proposed { background: var(--waiting-soft); color: var(--status-waiting); }
  .chip.t-running, .chip.t-doing, .chip.t-verifying { color: var(--status-working); }
  .chip.crit { background: var(--accent-soft); color: var(--fg); }
  .lanes { display: inline-flex; align-items: center; gap: 6px; font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); }
  .small { font-size: var(--text-2xs); }
  .empty { font-size: var(--text-sm); }

  .scroll { overflow: auto; max-height: 60vh; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); }
  .canvas { position: relative; }
  .wave-head {
    position: absolute; top: 0; height: 28px; display: flex; align-items: center; justify-content: center; gap: 6px;
    font-size: var(--text-2xs); font-weight: 600; color: var(--fg-2); border-bottom: 1px solid var(--border);
  }
  .lane { position: absolute; left: 0; right: 0; box-sizing: border-box; border-bottom: 1px solid var(--border); }
  .lane.alt { background: var(--bg-sunk); }
  .lane-head {
    height: 100%; display: flex; flex-direction: column; justify-content: center; gap: 2px;
    padding: 0 10px; box-sizing: border-box; border-right: 1px solid var(--border);
  }
  .lane-name { font-size: var(--text-xs); font-weight: 500; color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 112px; }
  .edges { position: absolute; inset: 0; pointer-events: none; }
  .edge { fill: none; stroke: var(--fg-muted); stroke-width: 1.25; stroke-dasharray: 3 3; opacity: 0.7; }
  .edge.met { stroke: var(--status-done); stroke-dasharray: none; opacity: 0.5; }
  .edge.crit { stroke: var(--accent); stroke-width: 2; stroke-dasharray: none; opacity: 1; }
  .edge.dim { opacity: 0.12; }

  .node {
    position: absolute; display: flex; align-items: center; gap: 6px; padding: 0 8px; box-sizing: border-box;
    border: 1px solid var(--control-border); border-radius: var(--radius-md); background: var(--bg-pane);
    color: var(--fg); text-align: left; cursor: pointer; font: inherit;
  }
  .node:hover { background: var(--bg-hover); }
  .node:focus-visible { outline: 2px solid var(--ring); outline-offset: 1px; }
  .node.crit { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  .node.sel { background: var(--accent-soft); }
  .node.dim { opacity: 0.35; }
  .node.t-done { border-left: 3px solid var(--status-done); }
  .node.t-working { border-left: 3px solid var(--status-working); }
  .node.t-waiting { border-left: 3px solid var(--status-waiting); background: var(--waiting-faint); }
  .node.t-failed { border-left: 3px solid var(--status-failed); }
  .node.t-idle { border-left: 3px solid var(--status-idle); }
  .text { display: flex; flex-direction: column; min-width: 0; }
  .text .title { font-size: var(--text-xs); line-height: 15px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .text .sub { font-size: var(--text-2xs); line-height: 14px; color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .chosen { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; font-size: var(--text-xs); padding: 6px 2px; }
  .chosen .title { font-weight: 500; }

  .node, .edge { transition: opacity var(--dur-fast) ease; }
</style>
