<!--
  Live lanes (automation runs board B): every run on one clock, a lane per
  routine, agent, mission and Jev use case. The line is now; a live run's
  bar grows toward it and shimmers. A routine fire that never started is a
  ring, a Jev decision a diamond. Picking a run opens its timeline.
-->
<script lang="ts">
  import { buildLanes, outcomeLabel, runMark, runSpan, type RunRow } from '../runs';

  let {
    runs,
    from,
    to,
    now,
    selected = null,
    onpick,
  }: {
    runs: RunRow[];
    from: number;
    to: number;
    now: number;
    selected?: string | null;
    onpick: (run: RunRow) => void;
  } = $props();

  const groups = $derived(buildLanes(runs, from, to));
  const span = $derived(Math.max(to - from, 1));
  const nowPct = $derived(Math.min(Math.max(((now - from) / span) * 100, 0), 100));

  /** A tick every "nice" step, about six across. */
  const STEPS = [900, 1800, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86400];
  const ticks = $derived.by(() => {
    const step = STEPS.find((s) => span / s <= 8) ?? 86400;
    const out: { at: number; pct: number; label: string }[] = [];
    const tz = new Date(from * 1000).getTimezoneOffset() * 60;
    let t = Math.ceil((from - tz) / step) * step + tz;
    for (; t <= to; t += step) {
      const d = new Date(t * 1000);
      const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
      const label = step >= 86400 ? d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' }) : hm;
      out.push({ at: t, pct: ((t - from) / span) * 100, label });
    }
    return out;
  });

  const clock = (sec: number) => {
    const d = new Date(sec * 1000);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  };
  function tip(r: RunRow): string {
    const end = r.ended_at ? `–${clock(r.ended_at)}` : r.outcome === 'running' ? '–now' : '';
    const why = r.summary ? `\n${r.summary}` : '';
    return `${r.owner} · ${clock(r.started_at)}${end}\n${outcomeLabel(r)}${r.host ? ` · ${r.host}` : ''}${why}`;
  }
</script>

<div class="lanes" data-testid="run-lanes">
  {#if groups.length === 0}
    <p class="none" data-testid="run-lanes-empty">Nothing ran in this window.</p>
  {:else}
    <div class="names">
      <div class="hrow lbl">Lane</div>
      {#each groups as g (g.group)}
        <div class="hrow group lbl">{g.group}</div>
        {#each g.lanes as l (l.key)}
          <div class="hrow lane-name" title={l.name}>
            <span class="xs nm">{l.name}</span>{#if l.host}<span class="xs muted">{l.host}</span>{/if}
          </div>
        {/each}
      {/each}
    </div>
    <div class="clock">
      <div class="hrow ticks xs muted tnum" aria-hidden="true">
        {#each ticks as t (t.at)}<span class="tick" style:left={`${t.pct}%`}>{t.label}</span>{/each}
      </div>
      {#each groups as g (g.group)}
        <div class="hrow group"></div>
        {#each g.lanes as l (l.key)}
          <div class="hrow lane" data-testid="run-lane" data-lane={l.key}>
            {#each l.runs as r (r.id)}
              {@const mark = runMark(r)}
              {@const s = runSpan(r, from, to, now)}
              {#if mark === 'bar'}
                <button
                  type="button"
                  class={`run ${r.outcome}`}
                  class:picked={selected === r.id}
                  style:left={`${s.left}%`}
                  style:width={`${s.width}%`}
                  title={tip(r)}
                  aria-label={`${r.owner}, ${outcomeLabel(r)}`}
                  data-testid="run-bar"
                  data-run={r.id}
                  onclick={() => onpick(r)}><span class="lab">{r.outcome === 'running' ? (r.summary ?? '') : ''}</span></button
                >
              {:else}
                <button
                  type="button"
                  class={`mark ${mark}`}
                  class:picked={selected === r.id}
                  style:left={`${s.left}%`}
                  title={tip(r)}
                  aria-label={`${r.owner}, ${mark === 'skip' ? 'skipped' : outcomeLabel(r)}`}
                  data-testid={`run-${mark}`}
                  data-run={r.id}
                  onclick={() => onpick(r)}
                ></button>
              {/if}
            {/each}
          </div>
        {/each}
      {/each}
      {#if nowPct > 0 && nowPct < 100}
        <div class="now" style:left={`${nowPct}%`} aria-hidden="true"><span class="xs tnum">now {clock(now)}</span></div>
      {/if}
    </div>
  {/if}
</div>
<div class="legend xs fg2" aria-hidden="true">
  <span><i class="sw running"></i>running</span>
  <span><i class="sw ok"></i>did work</span>
  <span><i class="sw nothing_to_do"></i>nothing to do</span>
  <span><i class="sw failed"></i>failed</span>
  <span><i class="sw needs_person"></i>needs you</span>
  <span><i class="ring"></i>skipped fire</span>
  <span><i class="dia"></i>Jev decided</span>
</div>

<style>
  .lanes { display: grid; grid-template-columns: 220px minmax(0, 1fr); background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); overflow: hidden; }
  .none { grid-column: 1 / -1; margin: 0; padding: var(--space-4); font-size: var(--text-sm); color: var(--fg-muted); }
  .names { border-right: 1px solid var(--border); min-width: 0; }
  .clock { position: relative; min-width: 0; }
  .hrow { height: 28px; display: flex; align-items: center; box-sizing: border-box; }
  .names .hrow { padding: 0 12px; gap: 8px; }
  .lbl { font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.06em; }
  .group { border-top: 1px solid var(--border); background: var(--bg-sunk); color: var(--fg-2); }
  .lane-name { padding-left: 20px !important; border-top: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  .lane { position: relative; border-top: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  .nm { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .xs { font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); }
  .fg2 { color: var(--fg-2); }
  .tnum { font-variant-numeric: tabular-nums; }
  .ticks { position: relative; }
  .tick { position: absolute; top: 6px; height: 22px; padding-left: 4px; border-left: 1px solid var(--border); white-space: nowrap; }
  .run { position: absolute; top: 6px; height: 16px; min-width: 4px; border: 0; padding: 0 6px; border-radius: var(--radius-sm); font: inherit; font-size: var(--text-2xs); line-height: 16px; text-align: left; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; cursor: pointer; background: var(--fg-muted); color: var(--fg); }
  .run.ok { background: color-mix(in srgb, var(--status-done) 45%, var(--bg-raise)); }
  .run.nothing_to_do { background: var(--border); }
  .run.failed { background: var(--status-failed); }
  .run.needs_person { background: var(--status-waiting); }
  .run.running { background: repeating-linear-gradient(90deg, var(--status-working) 0 14px, color-mix(in srgb, var(--status-working) 70%, transparent) 14px 24px); background-size: 24px 16px; color: var(--accent-fg); animation: stripes var(--loop-fast) linear infinite; box-shadow: 0 0 10px color-mix(in srgb, var(--status-working) 45%, transparent); }
  .run.picked, .mark.picked { outline: 2px solid var(--accent); outline-offset: 1px; }
  .lab { pointer-events: none; }
  .mark { position: absolute; top: 9px; width: 10px; height: 10px; margin-left: -5px; padding: 0; box-sizing: border-box; background: transparent; cursor: pointer; }
  .mark.skip { border-radius: 50%; border: 1.5px solid var(--fg-muted); }
  .mark.jev { transform: rotate(45deg); border: 1.5px solid var(--accent); }
  .now { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--accent); pointer-events: none; }
  .now span { position: absolute; top: 6px; left: 6px; color: var(--accent); font-weight: 600; white-space: nowrap; }
  .legend { display: flex; flex-wrap: wrap; gap: 6px 16px; align-items: center; }
  .legend span { display: inline-flex; align-items: center; gap: 6px; }
  .sw { width: 20px; height: 8px; border-radius: var(--radius-xs); display: inline-block; background: var(--fg-muted); }
  .sw.running { background: var(--status-working); }
  .sw.ok { background: color-mix(in srgb, var(--status-done) 45%, var(--bg-raise)); }
  .sw.nothing_to_do { background: var(--border); }
  .sw.failed { background: var(--status-failed); }
  .sw.needs_person { background: var(--status-waiting); }
  .ring { width: 10px; height: 10px; border-radius: 50%; border: 1.5px solid var(--fg-muted); box-sizing: border-box; display: inline-block; }
  .dia { width: 9px; height: 9px; transform: rotate(45deg); border: 1.5px solid var(--accent); box-sizing: border-box; display: inline-block; }
  @keyframes stripes { to { background-position: 24px 0; } }
  :global(:root[data-motion='reduced']) .run.running { animation: motion-fade var(--loader-reduced) ease-in-out infinite; }
  :global(:root[data-motion='off']) .run.running { animation: none; }
</style>
