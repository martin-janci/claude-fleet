<!--
  Work → Missions → Compare (lanes design 2026-10-23, L6): the missions that
  have run, side by side, with the median per execution mode on top, so the
  owner can tell whether running by lane makes more progress for its cost
  before switching the default. Pure rules in mission_compare.ts.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { formatDuration } from './account_usage';
  import { dollars } from './missions';
  import { closeMissionCompare, missionList, openMissionRoom, WAIT_LABEL } from './mission_room';
  import { loadCompare, rowTopWait, summarize, type CompareRow, type TopWait } from './mission_compare';

  let rows = $state<CompareRow[]>([]);
  let failed = $state(0);
  let loading = $state(true);

  async function load(): Promise<void> {
    loading = true;
    const r = await loadCompare(get(missionList).missions);
    rows = r.rows;
    failed = r.failed;
    loading = false;
  }

  onMount(() => {
    void load();
  });

  const summary = $derived(summarize(rows));
  const MODE: Record<string, string> = { item: 'Each in its own session', lane: 'By lane' };
  const num = (n: number | null, d = 1) => (n == null ? '–' : n.toFixed(d));
  const secs = (n: number | null) => (n == null ? '–' : formatDuration(n));
  const pct = (n: number | null) => (n == null ? '–' : `${Math.round(n)}%`);
  const usd = (n: number | null) => (n == null ? '–' : dollars(Math.round(n)));
  const wait = (w: TopWait | null) => (w ? `${WAIT_LABEL[w.cause] ?? w.cause} ${w.pct}%` : '–');
</script>

<div class="scroll">
  <div class="col" data-testid="mission-compare">
    <div class="head">
      <h2 class="name">Compare missions</h2>
      <span class="grow"></span>
      <button class="btn btn--quiet" type="button" disabled={loading} data-testid="mission-compare-reload" onclick={() => void load()}>Read again</button>
      <button class="btn btn--quiet" type="button" data-testid="mission-compare-close" onclick={closeMissionCompare}>Close</button>
    </div>
    <p class="muted small">
      The missions that have run, newest first. Per done task: fewer runs, fewer cold starts (sessions started) and a lower cost are
      better; idle is the share of the working span with no run open, and waited most is what held ready work back the longest
      (a person, a grant, a usage limit, a slot, a lane, overlapping files).
    </p>
    {#if loading}
      <p class="muted" data-testid="mission-compare-loading">Reading the missions…</p>
    {:else if rows.length === 0}
      <p class="muted" data-testid="mission-compare-empty">No mission has run yet.</p>
    {:else}
      <table class="grid" data-testid="mission-compare-summary">
        <caption class="muted small">Median per mode</caption>
        <thead>
          <tr><th>Tasks run</th><th>Missions</th><th>Done tasks</th><th>Runs / task</th><th>Cold starts / task</th><th>A task takes</th><th>Idle</th><th>Waited most</th><th>Cost / task</th></tr>
        </thead>
        <tbody>
          {#each summary as s (s.execution)}
            <tr data-testid="mission-compare-mode" data-mode={s.execution}>
              <th scope="row">{MODE[s.execution]}</th>
              <td>{s.missions}</td>
              <td>{s.doneTasks}</td>
              <td>{num(s.runsPerDone)}</td>
              <td>{num(s.coldStartsPerDone)}</td>
              <td>{secs(s.taskSecs)}</td>
              <td>{pct(s.idlePct)}</td>
              <td data-testid="mission-compare-wait">{wait(s.topWait)}</td>
              <td>{usd(s.costPerDoneMicros)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <table class="grid" data-testid="mission-compare-rows">
        <caption class="muted small">Each mission</caption>
        <thead>
          <tr><th>Mission</th><th>Tasks run</th><th>Done</th><th>Runs / task</th><th>Retries</th><th>Cold starts / task</th><th>A task takes</th><th>Idle</th><th>Waited most</th><th>Cost / task</th></tr>
        </thead>
        <tbody>
          {#each rows as r (r.id)}
            <tr data-testid="mission-compare-row">
              <th scope="row"
                ><button class="linkish" type="button" title="Open the mission" onclick={() => openMissionRoom(r.id)}>{r.name}</button>
                <span class="muted small">{r.state}</span></th
              >
              <td>{r.execution === 'lane' ? 'By lane' : 'Own session'}</td>
              <td>{r.done} / {r.total}</td>
              <td>{num(r.runsPerDone)}</td>
              <td>{r.retries}</td>
              <td>{num(r.coldStartsPerDone)}</td>
              <td>{secs(r.taskSecs)}</td>
              <td>{pct(r.idlePct)}</td>
              <td>{wait(rowTopWait(r))}</td>
              <td>{usd(r.costPerDoneMicros)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if failed > 0}
      <p class="muted small" role="status" data-testid="mission-compare-failed">{failed} mission{failed === 1 ? '' : 's'} could not be read.</p>
    {/if}
  </div>
</div>

<style>
  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    max-width: 960px;
    margin: 0 auto;
    padding: var(--space-2) var(--space-4) var(--space-4);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .name {
    font-size: var(--text-lg);
    margin: 0;
  }
  .grow {
    flex: 1;
  }
  .muted {
    color: var(--fg-muted);
  }
  .small {
    font-size: var(--text-xs);
  }
  .grid {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  .grid caption {
    text-align: left;
    padding-bottom: var(--space-1);
  }
  .grid th,
  .grid td {
    text-align: right;
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  .grid th[scope='row'],
  .grid thead th:first-child {
    text-align: left;
  }
  .linkish {
    background: none;
    border: 0;
    padding: 0;
    color: inherit;
    cursor: pointer;
    text-decoration: underline;
    font: inherit;
  }
</style>
