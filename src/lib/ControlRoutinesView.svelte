<!--
  Control's Routines view (gap plan G3.10, board MCTasks ◷): every routine
  with its state dot and line ("Weekdays · last run OK · next in 18h"),
  failed ones first. A row opens the routine in Automation, Run now starts
  one there and then; editing lives in Automation.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { listRoutines, lastRunByRoutine, openRoutines, routineDot, routineLine, runRoutineNow, type RoutineRow } from './routines';
  import { listRuns, type RunRow } from './runs';
  import { readErrorText } from './work_view';
  import type { IpcError } from './result';

  let routines = $state<RoutineRow[]>([]);
  let last = $state<Map<number, RunRow>>(new Map());
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  let note = $state<string | null>(null);
  let busy = $state<number | null>(null);

  async function load() {
    const [r, runs] = await Promise.all([listRoutines(), listRuns({ kind: 'routine', limit: 200 })]);
    loaded = true;
    if (!r.ok) {
      loadError = r.error;
      return;
    }
    loadError = null;
    routines = r.value;
    if (runs.ok && Array.isArray(runs.value?.runs)) last = lastRunByRoutine(runs.value.runs);
  }

  onMount(() => {
    void load();
  });

  const RANK: Record<string, number> = { failed: 0, waiting: 1, working: 2, done: 3, idle: 4 };
  const nowSec = Math.floor(Date.now() / 1000);
  const rows = $derived(
    routines
      .map((r) => ({ r, dot: routineDot(r, last.get(r.id)), line: routineLine(r, last.get(r.id), nowSec) }))
      .sort((a, b) => RANK[a.dot] - RANK[b.dot] || a.r.name.localeCompare(b.r.name)),
  );

  async function runNow(r: RoutineRow) {
    busy = r.id;
    const res = await runRoutineNow(r.id);
    busy = null;
    note = res.ok ? `${r.name} started.` : res.error.message;
    if (res.ok) void load();
  }
</script>

<div class="routines" data-testid="control-routines">
  {#if note}<p class="note" role="status" data-testid="control-routines-note">{note}</p>{/if}
  {#if loadError}
    <p class="muted" role="alert" data-testid="control-routines-error">
      {readErrorText(loadError)} <button type="button" class="btn" onclick={() => void load()}>Retry</button>
    </p>
  {:else if loaded && rows.length === 0}
    <div class="empty" data-testid="control-routines-empty">
      <p class="muted">No routines yet. A routine is a saved prompt that starts a session on a schedule or an event.</p>
      <button type="button" class="btn" data-testid="control-routines-template" onclick={() => openRoutines({ template: 'morning-pr-sweep' })}
        >Use a template</button
      >
    </div>
  {:else}
    <ul>
      {#each rows as { r, dot, line } (r.id)}
        <li class="row" data-testid="control-routine-row" data-state={dot}>
          <button type="button" class="open" title="Open in Automation" onclick={() => openRoutines({ select: r.id })}>
            <span class="name"><StatusDot state={dot} /> {r.name}</span>
            <span class="muted">{line}</span>
          </button>
          <button
            type="button"
            class="btn"
            aria-label="Run {r.name} now"
            data-testid="control-routine-run"
            disabled={busy !== null || !r.enabled}
            onclick={() => runNow(r)}>Run now</button
          >
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .routines {
    font-size: var(--text-sm);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .open {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
    text-align: left;
    border: 0;
    background: transparent;
    color: var(--fg);
    font: inherit;
    padding: 0;
    cursor: pointer;
  }
  .name {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }
  .btn {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 8px;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .open:focus-visible,
  .btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .note,
  .empty {
    margin: 0;
    padding: var(--space-2) var(--space-3);
  }
  .empty p {
    margin: 0 0 var(--space-2);
  }
</style>
