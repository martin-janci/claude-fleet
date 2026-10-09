<script lang="ts">
  // A small, dependency-free chart for a `series` data source: ONE series
  // (the source's first y column — two measures of different scale never
  // share an axis), so the title names it and no legend box is needed.
  // Bars: <= 24px, 4px rounded data end, square at the baseline, 2px surface
  // gap. Lines: 2px. Every point has a hover / focus tooltip, and "Table"
  // shows the same numbers as text, so nothing is gated on the picture.
  import { formatCell, type Column } from './pages';

  let {
    points,
    x,
    y,
    kind,
    title,
    limit = null,
    testid = 'chart',
  }: {
    points: Record<string, unknown>[];
    x: Column;
    y: Column;
    kind: 'line' | 'bar' | 'stacked_bar' | 'sparkline';
    title: string;
    /** A budget drawn as a dashed line across the plot, in the y column's
     *  unit; a bar above it is marked over. The line's words are in the
     *  caption, so the picture is never the only place it is said. */
    limit?: { value: number; label: string } | null;
    testid?: string;
  } = $props();

  const W = 560;
  const H = $derived(kind === 'sparkline' ? 40 : 160);
  const PAD_L = $derived(kind === 'sparkline' ? 0 : 44);
  const PAD_B = $derived(kind === 'sparkline' ? 0 : 20);
  const PAD_T = 8;

  const values = $derived(points.map((p) => Number(p[y.id]) || 0));
  const max = $derived(niceMax(Math.max(0, limit?.value ?? 0, ...values)));
  const over = (v: number) => limit !== null && limit.value > 0 && v > limit.value;
  const plotW = $derived(W - PAD_L);
  const plotH = $derived(H - PAD_B - PAD_T);
  const band = $derived(points.length ? plotW / points.length : plotW);
  const barW = $derived(Math.max(2, Math.min(24, band - 2)));

  function niceMax(v: number): number {
    if (v <= 0) return 1;
    const mag = 10 ** Math.floor(Math.log10(v));
    for (const m of [1, 2, 2.5, 5, 10]) if (m * mag >= v) return m * mag;
    return 10 * mag;
  }
  const yOf = (v: number) => PAD_T + plotH - (v / max) * plotH;
  const xMid = (i: number) => PAD_L + band * i + band / 2;

  /** A bar with a 4px rounded top and a square base. */
  function barPath(i: number, v: number): string {
    const h = Math.max(0, (v / max) * plotH);
    const x0 = xMid(i) - barW / 2;
    const base = PAD_T + plotH;
    const r = Math.min(4, barW / 2, h);
    if (h === 0) return '';
    return `M${x0},${base} V${base - h + r} Q${x0},${base - h} ${x0 + r},${base - h} H${x0 + barW - r} Q${x0 + barW},${base - h} ${x0 + barW},${base - h + r} V${base} Z`;
  }
  const linePath = $derived(
    values.map((v, i) => `${i === 0 ? 'M' : 'L'}${xMid(i)},${yOf(v)}`).join(' '),
  );

  let hover = $state<number | null>(null);
  let showTable = $state(false);
</script>

<figure class="chart" data-testid={testid}>
  <figcaption>
    <span>{title}{#if limit}<span class="limit-label" data-testid={`${testid}-limit`}> · {limit.label}</span>{/if}</span>
    {#if kind !== 'sparkline'}
      <button
        type="button"
        class="btn btn--quiet"
        aria-pressed={showTable}
        data-testid={`${testid}-table-toggle`}
        onclick={() => (showTable = !showTable)}>{showTable ? 'Chart' : 'Table'}</button
      >
    {/if}
  </figcaption>

  {#if points.length === 0}
    <p class="empty" data-testid={`${testid}-empty`}>No data yet.</p>
  {:else if showTable}
    <table class="data" data-testid={`${testid}-table`}>
      <thead><tr><th>{x.label}</th><th>{y.label}</th></tr></thead>
      <tbody>
        {#each points as p, i (i)}
          <tr><td>{formatCell(x.ty, p[x.id])}</td><td>{formatCell(y.ty, p[y.id])}</td></tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <div class="plot">
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={title}>
        {#if kind !== 'sparkline'}
          <line class="grid" x1={PAD_L} x2={W} y1={yOf(max)} y2={yOf(max)} />
          <line class="axis" x1={PAD_L} x2={W} y1={yOf(0)} y2={yOf(0)} />
          <text class="tick" x={PAD_L - 6} y={yOf(max) + 4} text-anchor="end"
            >{formatCell(y.ty, max)}</text
          >
          <text class="tick" x={PAD_L - 6} y={yOf(0)} text-anchor="end">0</text>
          <text class="tick" x={xMid(0)} y={H - 4} text-anchor="middle"
            >{formatCell(x.ty, points[0][x.id])}</text
          >
          {#if points.length > 1}
            <text class="tick" x={xMid(points.length - 1)} y={H - 4} text-anchor="middle"
              >{formatCell(x.ty, points[points.length - 1][x.id])}</text
            >
          {/if}
        {/if}
        {#if kind === 'bar' || kind === 'stacked_bar'}
          {#each values as v, i (i)}
            <path class="bar" class:over={over(v)} class:dim={hover !== null && hover !== i} d={barPath(i, v)} />
          {/each}
        {:else}
          <path class="line" d={linePath} />
          {#if hover !== null}
            <circle class="dot" cx={xMid(hover)} cy={yOf(values[hover])} r="4" />
          {/if}
        {/if}
        {#if limit && limit.value > 0 && kind !== 'sparkline'}
          <line class="limit" x1={PAD_L} x2={W} y1={yOf(limit.value)} y2={yOf(limit.value)} data-testid={`${testid}-limit-line`} />
        {/if}
        <!-- Hit targets: the whole band, bigger than the mark. -->
        {#each values as _, i (i)}
          <rect
            class="hit"
            x={PAD_L + band * i}
            y={PAD_T}
            width={band}
            height={plotH}
            role="presentation"
            onpointerenter={() => (hover = i)}
            onpointerleave={() => (hover = null)} />
        {/each}
      </svg>
      {#if hover !== null}
        <div
          class="tip"
          role="status"
          data-testid={`${testid}-tip`}
          style:left={`${(xMid(hover) / W) * 100}%`}>
          <strong>{formatCell(y.ty, values[hover])}</strong>
          <span>{formatCell(x.ty, points[hover][x.id])}</span>
        </div>
      {/if}
    </div>
    <!-- Keyboard: step through the points; the tip follows. -->
    <input
      class="scrub"
      type="range"
      min="0"
      max={points.length - 1}
      value={hover ?? points.length - 1}
      aria-label={`${title}: choose a ${x.label.toLowerCase()}`}
      oninput={(e) => (hover = Number((e.currentTarget as HTMLInputElement).value))}
      onblur={() => (hover = null)} />
  {/if}
</figure>

<style>
  .chart {
    margin: 0;
  }
  figcaption {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: var(--text-2xs);
    color: var(--fg);
    margin-bottom: 0.25rem;
  }
  .plot {
    position: relative;
  }
  svg {
    width: 100%;
    height: auto;
    display: block;
  }
  .grid,
  .axis {
    stroke: var(--border);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .tick {
    fill: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .bar {
    fill: var(--accent);
  }
  .bar.over {
    fill: var(--status-waiting);
  }
  .limit {
    stroke: var(--fg-muted);
    stroke-width: 1;
    stroke-dasharray: 4 3;
    vector-effect: non-scaling-stroke;
  }
  .limit-label {
    color: var(--fg-muted);
  }
  .bar.dim {
    opacity: 0.45;
  }
  .line {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
    vector-effect: non-scaling-stroke;
  }
  .dot {
    fill: var(--accent);
    stroke: var(--bg);
    stroke-width: 2;
  }
  .hit {
    fill: transparent;
  }
  .tip {
    position: absolute;
    top: 0;
    transform: translateX(-50%);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.2rem 0.4rem;
    font-size: var(--text-2xs);
    display: flex;
    gap: 0.4rem;
    pointer-events: none;
    white-space: nowrap;
  }
  .tip span {
    color: var(--fg-muted);
  }
  /* Keyboard stepping through the points: out of sight until focused. */
  .scrub {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  .scrub:focus-visible {
    position: static;
    width: 100%;
    height: auto;
    opacity: 1;
    margin: 0.25rem 0 0;
  }
  .empty {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  table.data {
    width: 100%;
    font-size: var(--text-2xs);
    border-collapse: collapse;
  }
  table.data th,
  table.data td {
    text-align: left;
    padding: 0.15rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
</style>
