<!--
  The orbit map (automation runs board C): is anything moving, and is
  anything wrong, at a glance. Planets are hosts, sized by their sessions;
  a comet is a live run circling its host (slow and dim once its session
  went quiet, its trail growing with how long it has run); an amber halo
  waits for you, a red spark failed in the last ten minutes; ticks on the
  outer ring are the next routine fires, clockwise from now at the top.
  A comet or planet click opens the run's timeline. CSS only.
-->
<script lang="ts">
  import { orbitModel, type OrbitModel } from '../orbit_map';
  import type { RunRow } from '../runs';

  let {
    runs,
    hostAliases,
    sessions,
    routines,
    now,
    onpick,
  }: {
    runs: RunRow[];
    hostAliases: string[];
    sessions: { id: number; host_alias: string; claude_status: string | null }[];
    routines: { name: string; enabled: boolean; trigger: string; host_alias: string; next_run_at?: number; skip_next?: boolean }[];
    now: number;
    onpick?: (run: RunRow) => void;
  } = $props();

  const model: OrbitModel = $derived(orbitModel({ runs, hostAliases, sessions, routines, now }));
  const byId = $derived(new Map(runs.map((r) => [r.id, r])));
  const clock = (sec: number) => {
    const d = new Date(sec * 1000);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  };
  function pick(id: string) {
    const r = byId.get(id);
    if (r) onpick?.(r);
  }
</script>

<div class="orbit" data-testid="orbit-map">
  <div class="stage" role="img" aria-label={`${model.running} running across ${model.hosts.length} hosts`}>
    <div class="ring outer"></div>
    {#each model.fires as f (f.routine + f.at)}
      <div class="arm" style:transform={`rotate(${f.deg}deg)`} title={`${f.routine} fires at ${f.label}`} data-testid="orbit-fire">
        <span class="tick"></span>
        <span class="tlabel xs tnum" style:transform={`rotate(${-f.deg}deg)`}>{f.label}</span>
      </div>
    {/each}
    <span class="nowlabel xs tnum">now {clock(now)}</span>

    <div class="ring hubring"></div>
    {#each Array(Math.min(model.jev, 6)) as _, i (i)}
      <span class="diamond" style:transform={`rotate(${i * 60 + 20}deg) translateY(-90px) rotate(45deg)`} data-testid="orbit-jev"></span>
    {/each}
    <div class="hub"><span class="xs">hub</span></div>

    {#each model.hosts as h (h.alias)}
      <div class="host" style:left={`${h.x}%`} style:top={`${h.y}%`} style:--s={h.scale} data-testid="orbit-host" data-host={h.alias}>
        <div class="ring hostring"></div>
        {#each h.comets as c (c.run_id)}
          <button
            type="button"
            class="comet"
            class:quiet={c.quiet}
            style:--lap={`${c.lap}s`}
            style:--trail={`${Math.round(c.trail * 100)}%`}
            title={`${c.owner}${c.quiet ? ' · going quiet' : ''}`}
            aria-label={`${c.owner}, running`}
            data-testid="orbit-comet"
            data-run={c.run_id}
            onclick={() => pick(c.run_id)}
          >
            <span class="trail"></span><span class="head"></span>
          </button>
        {/each}
        <div class="planet"><span class="xs">{h.alias}</span></div>
        {#if h.needs}<div class="halo" data-testid="orbit-halo"></div>{/if}
        {#if h.failed}<div class="spark" data-testid="orbit-spark"></div>{/if}
        <span class="caption xs fg2">{h.caption}</span>
      </div>
    {/each}
  </div>

  <div class="side">
    <div class="card">
      <span class="lbl">How to read it</span>
      <div class="legend xs fg2">
        <span class="lg planet-sw"></span><span>Planets are hosts; size is how many sessions run there.</span>
        <span class="lg comet-sw"></span><span>A comet is a live run. It slows and dims when its session goes quiet; its trail grows with how long it has run.</span>
        <span class="lg halo-sw"></span><span>Amber halo: a run waits for you.</span>
        <span class="lg spark-sw"></span><span>Red spark: a run failed in the last 10 minutes.</span>
        <span class="lg dia-sw"></span><span>Diamond on the hub ring: Jev decided something.</span>
        <span class="lg tick-sw"></span><span>Ticks on the outer ring are the next fires, clockwise from now at the top.</span>
      </div>
    </div>
    <div class="card" data-testid="orbit-feed">
      <div class="feed-head"><span class="lbl">Live</span><span class="chip">{model.running} running</span></div>
      {#if model.feed.length === 0}<span class="xs muted">Nothing ran recently.</span>{/if}
      {#each model.feed as e, i (i)}
        <div class="feed">
          <span class="xs muted tnum">{clock(e.at)}</span>
          <span class={`fdot ${e.tone}`}></span>
          <span class="xs"><b>{e.who}</b> <span class="fg2">{e.what}</span></span>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .orbit { display: grid; grid-template-columns: minmax(0, 560px) minmax(220px, 1fr); gap: 20px; align-items: start; }
  .stage { position: relative; width: 100%; aspect-ratio: 1; overflow: hidden; border-radius: var(--radius-lg); background: radial-gradient(circle at 50% 50%, color-mix(in srgb, var(--accent) 6%, var(--bg-sunk)), var(--bg-sunk) 70%); border: 1px solid var(--border); }
  .xs { font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); }
  .fg2 { color: var(--fg-2); }
  .tnum { font-variant-numeric: tabular-nums; }
  .lbl { font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.06em; }
  .ring { position: absolute; border-radius: 50%; border: 1px solid color-mix(in srgb, var(--fg) 8%, transparent); pointer-events: none; }
  .outer { inset: 4%; }
  .hubring { left: 32%; top: 32%; width: 36%; height: 36%; }
  .arm { position: absolute; left: 50%; top: 4%; width: 0; height: 46%; transform-origin: 0 100%; }
  .tick { position: absolute; left: -1px; top: -2px; width: 2px; height: 12px; background: var(--fg-muted); }
  .tlabel { position: absolute; left: 6px; top: 12px; color: var(--fg-muted); white-space: nowrap; }
  .nowlabel { position: absolute; left: calc(50% + 6px); top: 5%; color: var(--accent); font-weight: 600; }
  .hub { position: absolute; left: 50%; top: 50%; width: 10%; aspect-ratio: 1; transform: translate(-50%, -50%); border-radius: 50%; display: flex; align-items: center; justify-content: center; background: var(--bg-raise); border: 1px solid var(--border); color: var(--fg); }
  .diamond { position: absolute; left: calc(50% - 4px); top: calc(50% - 4px); width: 8px; height: 8px; box-sizing: border-box; border: 1.5px solid var(--accent); background: var(--accent-soft); }
  .host { position: absolute; width: calc(26% * var(--s)); aspect-ratio: 1; transform: translate(-50%, -50%); }
  .hostring { inset: 0; }
  .planet { position: absolute; inset: 27%; border-radius: 50%; display: flex; align-items: center; justify-content: center; background: radial-gradient(circle at 35% 30%, color-mix(in srgb, var(--accent) 30%, var(--bg-raise)), var(--bg-sunk)); border: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border)); color: var(--fg); overflow: hidden; }
  .halo { position: absolute; inset: 24%; border-radius: 50%; border: 2px solid var(--status-waiting); pointer-events: none; animation: pulse var(--loop-slow) ease-in-out infinite; }
  .spark { position: absolute; right: 22%; top: 22%; width: 10px; height: 10px; border-radius: 50%; background: var(--status-failed); box-shadow: 0 0 8px 2px color-mix(in srgb, var(--status-failed) 60%, transparent); animation: pulse var(--loop-fast) ease-in-out infinite; }
  .caption { position: absolute; top: calc(100% + 2px); left: -30%; right: -30%; text-align: center; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .comet { position: absolute; inset: 0; padding: 0; border: 0; border-radius: 50%; background: transparent; cursor: pointer; animation: orbit var(--lap) linear infinite; }
  .comet.quiet { opacity: 0.45; }
  .trail { position: absolute; inset: 0; border-radius: 50%; background: conic-gradient(from 0deg, transparent 0 calc(100% - var(--trail)), color-mix(in srgb, var(--status-working) 90%, transparent) 100%); mask: radial-gradient(farthest-side, transparent calc(100% - 3px), var(--bg) calc(100% - 2px)); -webkit-mask: radial-gradient(farthest-side, transparent calc(100% - 3px), var(--bg) calc(100% - 2px)); }
  .head { position: absolute; left: calc(50% - 4px); top: -3px; width: 8px; height: 8px; border-radius: 50%; background: var(--fg); box-shadow: 0 0 10px 3px var(--status-working); }

  .side { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
  .card { background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); padding: 12px 14px; display: flex; flex-direction: column; gap: 8px; }
  .legend { display: grid; grid-template-columns: 18px minmax(0, 1fr); gap: 8px 10px; align-items: center; }
  .lg { justify-self: center; display: inline-block; }
  .planet-sw { width: 14px; height: 14px; border-radius: 50%; background: var(--bg-raise); border: 1px solid var(--border); }
  .comet-sw { width: 8px; height: 8px; border-radius: 50%; background: var(--status-working); box-shadow: 0 0 6px 2px color-mix(in srgb, var(--status-working) 60%, transparent); }
  .halo-sw { width: 10px; height: 10px; border-radius: 50%; border: 2px solid var(--status-waiting); }
  .spark-sw { width: 8px; height: 8px; border-radius: 50%; background: var(--status-failed); }
  .dia-sw { width: 8px; height: 8px; transform: rotate(45deg); border: 1.5px solid var(--accent); }
  .tick-sw { width: 2px; height: 12px; background: var(--fg-muted); }
  .feed-head { display: flex; align-items: center; }
  .feed-head .lbl { flex: 1; }
  .chip { height: 20px; padding: 0 7px; border-radius: var(--radius-xs); font-size: var(--text-2xs); display: inline-flex; align-items: center; background: var(--bg-raise); color: var(--fg-2); }
  .feed { display: grid; grid-template-columns: 40px 8px minmax(0, 1fr); gap: 8px; align-items: baseline; }
  .feed b { font-weight: 500; }
  .fdot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg-muted); }
  .fdot.working { background: var(--status-working); }
  .fdot.waiting { background: var(--status-waiting); }
  .fdot.failed { background: var(--status-failed); }
  .fdot.done { background: var(--status-done); }
  .fdot.jev { background: var(--accent); }

  /* A comet's lap round its host: an orbit, not a loading spinner (the
     Loader kit draws those). */
  @keyframes orbit { to { transform: rotate(1turn); } }
  @keyframes pulse { 50% { opacity: 0.5; } }
  /* Reduced motion: comets stand still and the halo and spark fade once a
     beat; Off holds everything still (motion.ts sets data-motion). */
  :global(:root[data-motion='reduced']) .comet { animation: none; }
  :global(:root[data-motion='reduced']) .halo,
  :global(:root[data-motion='reduced']) .spark { animation: motion-fade var(--loader-reduced) ease-in-out infinite; }
  :global(:root[data-motion='off']) .comet,
  :global(:root[data-motion='off']) .halo,
  :global(:root[data-motion='off']) .spark { animation: none; }
  @media (max-width: 900px) { .orbit { grid-template-columns: minmax(0, 1fr); } }
</style>
