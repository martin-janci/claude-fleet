<!--
  A compact task graph in Control's chat (Thread room, boards Main and
  A3Mission): the steps of a mission or of a proposed tree, wave by wave,
  each with its state, the step it is at now outlined, and "Open full
  graph" (the mission's full MissionGraph in Control's Missions view, or a
  tree's parent task in Work). Read-only, like the full graph.
-->
<script lang="ts">
  import StatusDot from './kit/StatusDot.svelte';
  import { STEPS_PER_WAVE, graphProgress, type GraphCard } from './task_graph_card';

  let { graph, kind, onopen }: { graph: GraphCard; kind: 'mission' | 'tree'; onopen: () => void } = $props();
</script>

<div class="graph-card" data-testid="task-graph-card" data-kind={kind}>
  <div class="head">
    <span class="title">Task graph · {graph.title}</span>
    <span class="progress" data-testid="task-graph-progress">{graphProgress(graph)}</span>
    <button type="button" class="btn btn--quiet open" data-testid="task-graph-open" onclick={onopen}>Open full graph</button>
  </div>
  <ol class="waves" aria-label="Steps by wave">
    {#each graph.waves as wave, w (w)}
      {#if w > 0}<li class="arrow" aria-hidden="true">→</li>{/if}
      <li class="wave">
        <ul class="steps">
          {#each wave.slice(0, STEPS_PER_WAVE) as s (s.id)}
            <li class="step" class:current={s.current} data-testid="task-graph-step" data-tone={s.tone}>
              <span class="step-head"><StatusDot state={s.tone} size={7} /><span class="step-title" title={s.title}>{s.title}</span></span>
              <span class="step-sub">{s.sub}</span>
            </li>
          {/each}
          {#if wave.length > STEPS_PER_WAVE}
            <li class="more">+{wave.length - STEPS_PER_WAVE} more</li>
          {/if}
        </ul>
      </li>
    {/each}
  </ol>
</div>

<style>
  .graph-card {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .title {
    flex: 1 1 auto;
    min-width: 0;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .progress {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .open {
    height: 20px;
  }
  .waves,
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .waves {
    display: flex;
    gap: 6px;
    align-items: stretch;
    overflow-x: auto;
  }
  .wave {
    flex: 1 1 0;
    min-width: 96px;
  }
  .steps {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .arrow {
    flex: none;
    align-self: center;
    color: var(--control-border);
  }
  .step {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
    min-width: 0;
  }
  .step.current {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .step-head {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .step-title {
    font-size: var(--text-xs);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .step-sub,
  .more {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    line-height: var(--text-2xs-lh);
  }
</style>
