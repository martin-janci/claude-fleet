<script lang="ts">
  // A burn-up (persona dashboards, step 8): the query's tasks and how many
  // were done, day by day, from `work_stats { since }`. Two series of one
  // unit share one axis. Scope is a muted dashed line and done the accent,
  // so the two differ by more than colour; the legend names both and each
  // line ends in its value. Every day has a hover / focus tooltip with both
  // numbers, and "Table" shows the same numbers as text.
  import type { BurnupDay } from './dashboards';

  let { days, title, testid = 'burnup' }: { days: BurnupDay[]; title: string; testid?: string } = $props();

  const W = 560;
  const H = 160;
  const PAD_L = 36;
  const PAD_R = 44;
  const PAD_B = 20;
  const PAD_T = 10;
  const plotW = W - PAD_L - PAD_R;
  const plotH = H - PAD_B - PAD_T;

  function niceMax(v: number): number {
    if (v <= 0) return 1;
    const mag = 10 ** Math.floor(Math.log10(v));
    for (const m of [1, 2, 2.5, 5, 10]) if (m * mag >= v) return m * mag;
    return 10 * mag;
  }
  const max = $derived(niceMax(Math.max(0, ...days.map((d) => d.total))));
  const xOf = (i: number) => PAD_L + (days.length > 1 ? (plotW * i) / (days.length - 1) : plotW / 2);
  const yOf = (v: number) => PAD_T + plotH - (v / max) * plotH;
  const path = (vals: number[]) => vals.map((v, i) => `${i === 0 ? 'M' : 'L'}${xOf(i)},${yOf(v)}`).join(' ');
  const scopePath = $derived(path(days.map((d) => d.total)));
  const donePath = $derived(path(days.map((d) => d.done)));
  const last = $derived(days.at(-1));

  const dayText = (unix: number) =>
    new Date(unix * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', timeZone: 'UTC' });

  let hover = $state<number | null>(null);
  let showTable = $state(false);
</script>

<figure class="chart" data-testid={testid}>
  <figcaption>
    <span class="legend">
      <span class="key"><i class="sw done" aria-hidden="true"></i>Done</span>
      <span class="key"><i class="sw scope" aria-hidden="true"></i>Scope</span>
    </span>
    <button
      type="button"
      class="toggle"
      aria-pressed={showTable}
      data-testid={`${testid}-table-toggle`}
      onclick={() => (showTable = !showTable)}>{showTable ? 'Chart' : 'Table'}</button
    >
  </figcaption>

  {#if days.length === 0}
    <p class="empty" data-testid={`${testid}-empty`}>No history yet.</p>
  {:else if showTable}
    <table class="data" data-testid={`${testid}-table`}>
      <thead><tr><th>Day</th><th class="num">Done</th><th class="num">Scope</th></tr></thead>
      <tbody>
        {#each days as d (d.day)}
          <tr><td>{dayText(d.day)}</td><td class="num">{d.done}</td><td class="num">{d.total}</td></tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <div class="plot">
      <svg
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={last ? `${title}: ${last.done} of ${last.total} done on ${dayText(last.day)}` : title}>
        <line class="grid" x1={PAD_L} x2={W - PAD_R} y1={yOf(max)} y2={yOf(max)} />
        <line class="axis" x1={PAD_L} x2={W - PAD_R} y1={yOf(0)} y2={yOf(0)} />
        <text class="tick" x={PAD_L - 6} y={yOf(max) + 4} text-anchor="end">{max}</text>
        <text class="tick" x={PAD_L - 6} y={yOf(0)} text-anchor="end">0</text>
        <text class="tick" x={xOf(0)} y={H - 4} text-anchor="start">{dayText(days[0].day)}</text>
        {#if days.length > 1}
          <text class="tick" x={xOf(days.length - 1)} y={H - 4} text-anchor="end">{dayText(days[days.length - 1].day)}</text>
        {/if}
        <path class="line scope" d={scopePath} />
        <path class="line done" d={donePath} />
        {#if last}
          <!-- Direct labels at the line ends. -->
          <text class="end" x={xOf(days.length - 1) + 6} y={yOf(last.total) + 4}>{last.total}</text>
          {#if last.done !== last.total}
            <text class="end" x={xOf(days.length - 1) + 6} y={yOf(last.done) + 4}>{last.done}</text>
          {/if}
        {/if}
        {#if hover !== null}
          <line class="cross" x1={xOf(hover)} x2={xOf(hover)} y1={PAD_T} y2={yOf(0)} />
          <circle class="dot scope" cx={xOf(hover)} cy={yOf(days[hover].total)} r="4" />
          <circle class="dot done" cx={xOf(hover)} cy={yOf(days[hover].done)} r="4" />
        {/if}
        <!-- Hit targets: a band per day, bigger than the mark. -->
        {#each days as d, i (d.day)}
          <rect
            class="hit"
            x={xOf(i) - plotW / Math.max(1, days.length - 1) / 2}
            y={PAD_T}
            width={plotW / Math.max(1, days.length - 1)}
            height={plotH}
            role="presentation"
            onpointerenter={() => (hover = i)}
            onpointerleave={() => (hover = null)} />
        {/each}
      </svg>
      {#if hover !== null}
        <div class="tip" role="status" data-testid={`${testid}-tip`} style:left={`${(xOf(hover) / W) * 100}%`}>
          <span>{dayText(days[hover].day)}</span>
          <strong>{days[hover].done} of {days[hover].total} done</strong>
        </div>
      {/if}
    </div>
    <input
      class="scrub"
      type="range"
      min="0"
      max={days.length - 1}
      value={hover ?? days.length - 1}
      aria-label={`${title}: choose a day`}
      oninput={(e) => (hover = Number((e.currentTarget as HTMLInputElement).value))}
      onblur={() => (hover = null)} />
  {/if}
</figure>

<style>
  .chart { margin: 0; }
  figcaption { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 4px; font-size: var(--text-2xs); color: var(--fg-2); }
  .legend { display: inline-flex; gap: 12px; }
  .key { display: inline-flex; align-items: center; gap: 4px; }
  .sw { display: inline-block; width: 12px; height: 0; border-top: 2px solid var(--accent); }
  .sw.scope { border-top: 2px dashed var(--fg-muted); }
  .toggle { font: inherit; font-size: var(--text-2xs); color: var(--fg-muted); background: none; border: none; padding: 0; cursor: pointer; }
  .toggle:hover { color: var(--fg); text-decoration: underline; }
  .plot { position: relative; }
  svg { width: 100%; height: auto; display: block; }
  .grid, .axis { stroke: var(--border); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .tick, .end { fill: var(--fg-muted); font-size: var(--text-2xs); }
  .end { fill: var(--fg-2); }
  .line { fill: none; stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; vector-effect: non-scaling-stroke; }
  .line.done { stroke: var(--accent); }
  .line.scope { stroke: var(--fg-muted); stroke-dasharray: 5 4; }
  .cross { stroke: var(--border); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .dot { stroke: var(--bg-pane); stroke-width: 2; }
  .dot.done { fill: var(--accent); }
  .dot.scope { fill: var(--fg-muted); }
  .hit { fill: transparent; }
  .tip { position: absolute; top: 0; transform: translateX(-50%); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 2px 6px; font-size: var(--text-2xs); display: flex; gap: 6px; pointer-events: none; white-space: nowrap; }
  .tip span { color: var(--fg-muted); }
  .scrub { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
  .scrub:focus-visible { position: static; width: 100%; height: auto; opacity: 1; margin: 4px 0 0; }
  .empty { margin: 0; font-size: var(--text-sm); color: var(--fg-muted); }
  table.data { width: 100%; font-size: var(--text-2xs); border-collapse: collapse; }
  table.data th, table.data td { text-align: left; padding: 2px 6px; border-bottom: 1px solid var(--border); }
  table.data .num { text-align: right; font-variant-numeric: tabular-nums; }
</style>
