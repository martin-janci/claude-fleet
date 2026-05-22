<script lang="ts" module>
  // Stable lane palette, indexed by GraphRow.color.
  const PALETTE = [
    '#58a6ff', '#3fb950', '#d29922', '#db61a2', '#a371f7',
    '#f85149', '#39c5cf', '#e3b341', '#bc8cff', '#7ee787',
  ];
  export function laneColor(i: number): string {
    return PALETTE[i % PALETTE.length];
  }
</script>

<script lang="ts">
  import type { Commit } from './history';
  import { computeGraph } from './graph';

  let {
    commits,
    selected,
    onSelect,
    onCreateBranch,
    onCheckoutCommit,
  }: {
    commits: Commit[];
    selected: string | null;
    onSelect: (hash: string) => void;
    onCreateBranch: (hash: string) => void;
    onCheckoutCommit: (hash: string) => void;
  } = $props();

  const rows = $derived(computeGraph(commits.map((c) => ({ hash: c.hash, parents: c.parents }))));
  const byHash = $derived(new Map(commits.map((c) => [c.hash, c])));

  // Geometry for the SVG gutter.
  const ROW_H = 26;
  const COL_W = 14;
  const DOT_R = 4;
  const cx = (col: number) => 8 + col * COL_W;

  const maxLanes = $derived(
    rows.reduce((m, r) => Math.max(m, r.lanesIn.length, r.lanesOut.length), 1),
  );
  const gutterW = $derived(cx(maxLanes) + 8);

  // For each row build the line segments to draw inside its cell:
  // top-half (lanesIn → dot/continuation) and bottom-half (dot/continuation → lanesOut).
  function segments(i: number): { x1: number; y1: number; x2: number; y2: number; color: string }[] {
    const r = rows[i];
    const segs: { x1: number; y1: number; x2: number; y2: number; color: string }[] = [];
    const midY = ROW_H / 2;
    const dotX = cx(r.column);
    // top half: each incoming lane goes to its continuation (same hash in lanesOut) or to the dot.
    r.lanesIn.forEach((h, col) => {
      if (h === null) return;
      const color = laneColor(r.colors[h] ?? r.color);
      if (h === r.hash) {
        segs.push({ x1: cx(col), y1: 0, x2: dotX, y2: midY, color });
      } else {
        const out = r.lanesOut.indexOf(h);
        if (out !== -1) segs.push({ x1: cx(col), y1: 0, x2: cx(out), y2: midY, color });
      }
    });
    // bottom half: dot → first-parent continuation; plus any new parent lanes.
    r.lanesOut.forEach((h, col) => {
      if (h === null) return;
      const color = laneColor(r.colors[h] ?? r.color);
      const cameFrom = r.lanesIn.indexOf(h);
      if (cameFrom === -1) {
        // a parent introduced by this commit → draw from the dot.
        segs.push({ x1: dotX, y1: midY, x2: cx(col), y2: ROW_H, color });
      } else {
        // lane passing through → straight segment bottom half.
        segs.push({ x1: cx(col), y1: midY, x2: cx(col), y2: ROW_H, color });
      }
    });
    return segs;
  }

  function rel(date: string): string {
    const t = Date.parse(date);
    if (Number.isNaN(t)) return date;
    const s = Math.floor((Date.now() - t) / 1000);
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  }
</script>

<div class="graph" data-testid="commit-graph">
  {#each rows as r, i (r.hash)}
    {@const c = byHash.get(r.hash)}
    <div
      class="crow"
      class:sel={selected === r.hash}
      role="button"
      tabindex="0"
      onclick={() => onSelect(r.hash)}
      onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && onSelect(r.hash)}
    >
      <svg class="gutter" width={gutterW} height={ROW_H} aria-hidden="true">
        {#each segments(i) as s}
          <line x1={s.x1} y1={s.y1} x2={s.x2} y2={s.y2} stroke={s.color} stroke-width="1.5" />
        {/each}
        <circle cx={cx(r.column)} cy={ROW_H / 2} r={DOT_R} fill={laneColor(r.color)} />
      </svg>
      <span class="meta">
        {#each c?.refs ?? [] as ref}
          <span class="ref {ref.kind}">{ref.name}</span>
        {/each}
        <span class="subject" title={c?.subject}>{c?.subject}</span>
        <span class="author">{c?.author}</span>
        <span class="date">{c ? rel(c.date) : ''}</span>
      </span>
      <span class="actions">
        <button
          title="Create branch from here"
          onclick={(e) => { e.stopPropagation(); onCreateBranch(r.hash); }}>⎇</button
        >
        <button
          title="Checkout this commit (detached)"
          onclick={(e) => { e.stopPropagation(); onCheckoutCommit(r.hash); }}>⤓</button
        >
      </span>
    </div>
  {/each}
</div>

<style>
  .graph { font-size: 0.78rem; }
  .crow {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    height: 26px;
    background: transparent;
    border: none;
    color: var(--fg);
    cursor: pointer;
    padding: 0 0.4rem;
    text-align: left;
  }
  .crow:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .crow.sel { background: color-mix(in srgb, var(--accent) 22%, transparent); }
  .crow:hover .actions { visibility: visible; }
  .gutter { flex: 0 0 auto; }
  .meta {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    overflow: hidden;
  }
  .subject {
    flex: 1 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .author, .date { flex: 0 0 auto; color: var(--fg-muted); font-size: 0.72rem; }
  .ref {
    flex: 0 0 auto;
    border-radius: 3px;
    padding: 0 0.3rem;
    font-size: 0.66rem;
    font-family: var(--mono, monospace);
  }
  .ref.branch { background: color-mix(in srgb, #3fb950 30%, transparent); color: #3fb950; }
  .ref.remote { background: color-mix(in srgb, #58a6ff 28%, transparent); color: #58a6ff; }
  .ref.tag { background: color-mix(in srgb, #d29922 30%, transparent); color: #d29922; }
  .ref.head { background: color-mix(in srgb, #f85149 30%, transparent); color: #f85149; }
  .actions { flex: 0 0 auto; visibility: hidden; display: flex; gap: 0.2rem; }
  .actions button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0 0.3rem;
  }
  .actions button:hover { color: var(--fg); border-color: var(--accent); }
</style>
