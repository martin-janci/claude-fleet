<!--
  The orbit map (automation runs board C): is anything moving, and is
  anything wrong, at a glance. Planets are hosts, sized by their sessions,
  their number inside and their name beneath; a comet is a live run circling
  its host (slow and dim once its session went quiet, its trail growing
  with how long it has run); small satellites are sessions at work; moons
  on a host's ring are the last hour's finished runs, coloured by how they
  ended; a host that ran something lately ripples and its spoke to the hub
  pulses; an amber halo waits for you, a red spark failed in the last ten
  minutes. The outer ring is a day with now at the top: next fires tick
  clockwise down the right, past runs dot anticlockwise down the left.
  A comet or moon click opens the run's timeline. CSS only. `compact`
  stacks the live feed under the stage and drops the legend, for Control's
  side panel.
-->
<script lang="ts">
  import { clockLabel, orbitModel, orbitStageLabel, type OrbitModel } from '../orbit_map';
  import { countRoll, toastIn } from '../motion_catalog';
  import type { RunRow } from '../runs';

  let {
    runs,
    hostAliases,
    sessions,
    routines,
    now,
    onpick,
    compact = false,
  }: {
    runs: RunRow[];
    hostAliases: string[];
    sessions: { id: number; host_alias: string; claude_status: string | null }[];
    routines: { name: string; enabled: boolean; trigger: string; host_alias: string; next_run_at?: number; skip_next?: boolean }[];
    now: number;
    onpick?: (run: RunRow) => void;
    compact?: boolean;
  } = $props();

  const model: OrbitModel = $derived(orbitModel({ runs, hostAliases, sessions, routines, now }));
  const byId = $derived(new Map(runs.map((r) => [r.id, r])));
  function pick(id: string) {
    const r = byId.get(id);
    if (r) onpick?.(r);
  }
</script>

<div class="orbit" class:compact data-testid="orbit-map">
  <div
    class="stage"
    role="group"
    aria-label={orbitStageLabel(model)}
  >
    <div class="ring outer"></div>
    <svg class="spokes" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
      {#each model.hosts as h (h.alias)}
        <line
          x1="50"
          y1="50"
          x2={h.x}
          y2={h.y}
          class="spoke"
          class:live={h.busy}
          class:warm={h.heat > 0}
          style:--heat={h.heat}
        />
      {/each}
    </svg>
    {#each model.fires as f (f.routine + f.at)}
      <div class="arm" style:transform={`rotate(${f.deg}deg)`} title={`${f.routine} fires at ${f.label}`} data-testid="orbit-fire">
        <span class="tick"></span>
        <span class="tlabel xs tnum" style:transform={`rotate(${-f.deg}deg)`}>{f.label}</span>
      </div>
    {/each}
    {#each model.past as p (p.run_id)}
      <div class="arm" style:transform={`rotate(${p.deg}deg)`} data-testid="orbit-past">
        <span class={`pdot ${p.tone}`} title={p.title}></span>
      </div>
    {/each}
    <span class="nowmark" aria-hidden="true"></span>
    <span class="nowlabel xs tnum">now {clockLabel(now)}</span>
    <span class="half past xs">past 12h</span>
    <span class="half next xs">next 12h</span>

    <div class="ring hubring"></div>
    {#each Array(Math.min(model.jev, 6)) as _, i (i)}
      <span class="diamond" style:transform={`rotate(${i * 60 + 20}deg) translateY(-12cqw) rotate(45deg)`} data-testid="orbit-jev"></span>
    {/each}
    <div class="hub" class:warm={model.hubHeat > 0} style:--heat={model.hubHeat}>
      {#if model.hubHeat > 0}<span class="ripple" data-testid="orbit-hub-ripple"></span>{/if}
      <span class="xs">hub</span>
    </div>

    {#each model.hosts as h (h.alias)}
      <div
        class="host"
        class:busy={h.busy}
        style:left={`${h.x}%`}
        style:top={`${h.y}%`}
        style:--s={h.scale}
        style:--heat={h.heat}
        data-testid="orbit-host"
        data-host={h.alias}
      >
        <div class="ring hostring"></div>
        {#if h.heat > 0}<span class="ripple" data-testid="orbit-ripple"></span>{/if}
        {#each Array(h.working) as _, k (k)}
          <span class="sat" style:--phase={k / h.working} data-testid="orbit-satellite"><span class="sathead"></span></span>
        {/each}
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
        {#each h.moons as m (m.run_id)}
          <button
            type="button"
            class={`moon ${m.tone}`}
            style:left={`${m.x}%`}
            style:top={`${m.y}%`}
            title={m.title}
            aria-label={m.title}
            data-testid="orbit-moon"
            data-run={m.run_id}
            onclick={() => pick(m.run_id)}
          ></button>
        {/each}
        <div class="planet" title={`${h.alias} · ${h.sessions} session${h.sessions === 1 ? '' : 's'}`}>
          {#if h.sessions > 0}<span class="n tnum">{h.sessions}</span>{/if}
        </div>
        {#if h.needs}<div class="halo" data-testid="orbit-halo"></div>{/if}
        {#if h.failed}<div class="spark" data-testid="orbit-spark"></div>{/if}
        <div class="label">
          <span class="name" title={h.alias} data-testid="orbit-host-name">{h.label}</span>
          <span class="caption fg2" title={h.caption}>{h.caption}</span>
        </div>
      </div>
    {/each}
  </div>

  <div class="side">
    {#if !compact}
    <div class="card">
      <span class="lbl">How to read it</span>
      <div class="legend xs fg2">
        <span class="lg planet-sw"></span><span>Planets are hosts; size and the number inside are how many sessions run there.</span>
        <span class="lg comet-sw"></span><span>A comet is a live run. It slows and dims when its session goes quiet; its trail grows with how long it has run.</span>
        <span class="lg sat-sw"></span><span>A satellite is a session at work on that host.</span>
        <span class="lg moon-sw"></span><span>Moons are the last hour's finished runs, coloured by how they ended. Click one for its steps.</span>
        <span class="lg ripple-sw"></span><span>A ripple, and a pulse on its spoke: the host ran something in the last 15 minutes.</span>
        <span class="lg halo-sw"></span><span>Amber halo: a run waits for you.</span>
        <span class="lg spark-sw"></span><span>Red spark: a run failed in the last 10 minutes.</span>
        <span class="lg dia-sw"></span><span>Diamond on the hub ring: Jev decided something.</span>
        <span class="lg tick-sw"></span><span>The outer ring is a day, now at the top: next fires tick down the right, past runs dot down the left.</span>
      </div>
    </div>
    {/if}
    <div class="card" data-testid="orbit-feed">
      <div class="feed-head">
        <span class="lbl">Live</span>
        {#if model.working > 0}<span class="chip working" data-testid="orbit-working"><span class="fdot working"></span><span use:countRoll={model.working}>{model.working}</span> working</span>{/if}
        <span class="chip" class:on={model.running > 0}><span use:countRoll={model.running}>{model.running}</span> running</span>
      </div>
      {#if model.feed.length === 0}<span class="xs muted">Nothing ran recently.</span>{/if}
      {#each model.feed as e (e.run_id)}
        <div class="feed" in:toastIn>
          <span class="xs muted tnum">{clockLabel(e.at)}</span>
          <span class={`fdot ${e.tone}`}></span>
          <span class="xs what" title={`${e.who} ${e.what}`}><b>{e.who}</b> <span class="fg2">{e.what}</span></span>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .orbit { display: grid; grid-template-columns: minmax(0, 560px) minmax(220px, 1fr); gap: 20px; align-items: start; }
  /* A size container, so labels and marks scale with the stage, not the window. */
  .stage { position: relative; width: 100%; aspect-ratio: 1; container-type: size; overflow: hidden; border-radius: var(--radius-lg); background: radial-gradient(circle at 50% 50%, color-mix(in srgb, var(--accent) 6%, var(--bg-sunk)), var(--bg-sunk) 70%); border: 1px solid var(--border); }
  .xs { font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); }
  .fg2 { color: var(--fg-2); }
  .tnum { font-variant-numeric: tabular-nums; }
  .lbl { font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.06em; }
  .ring { position: absolute; border-radius: 50%; border: 1px solid color-mix(in srgb, var(--fg) 8%, transparent); pointer-events: none; }
  .outer { inset: 4%; }
  .hubring { left: 38%; top: 38%; width: 24%; height: 24%; border-style: dashed; }

  /* Spokes: hub to host. A host at work carries a flow; one that ran lately glows. */
  .spokes { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; overflow: visible; }
  .spoke { stroke: color-mix(in srgb, var(--fg) 10%, transparent); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .spoke.warm { stroke: color-mix(in srgb, var(--accent) calc(25% + 55% * var(--heat)), transparent); }
  .spoke.live { stroke: color-mix(in srgb, var(--status-working) 70%, transparent); stroke-width: 1.5; stroke-dasharray: 2 5; animation: flow var(--loop-slow) linear infinite; }

  /* The outer ring's arms: a fire's tick on the right, a past run's dot on the left. */
  .arm { position: absolute; left: 50%; top: 4%; width: 0; height: 46%; transform-origin: 0 100%; pointer-events: none; }
  .tick { position: absolute; left: -1px; top: -2px; width: 2px; height: 12px; background: var(--fg-muted); pointer-events: auto; }
  .tlabel { position: absolute; left: 6px; top: 12px; color: var(--fg-muted); white-space: nowrap; }
  .pdot { position: absolute; left: -3px; top: -3px; width: 6px; height: 6px; border-radius: 50%; background: var(--fg-muted); pointer-events: auto; }
  .pdot.done { background: var(--status-done); }
  .pdot.failed { background: var(--status-failed); }
  .pdot.waiting { background: var(--status-waiting); }
  .pdot.jev { background: var(--accent); }
  .pdot.idle { background: color-mix(in srgb, var(--fg-muted) 60%, transparent); }
  .nowmark { position: absolute; left: calc(50% - 4px); top: calc(4% - 4px); width: 8px; height: 8px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 60%, transparent); animation: beacon var(--loop-slow) ease-out infinite; }
  .nowlabel { position: absolute; left: 0; right: 0; top: calc(4% + 8px); text-align: center; color: var(--accent); font-weight: 600; pointer-events: none; }
  .half { position: absolute; bottom: 1%; color: var(--fg-muted); opacity: 0.7; pointer-events: none; }
  .half.past { right: calc(50% + 8px); }
  .half.next { left: calc(50% + 8px); }

  .hub { position: absolute; left: 50%; top: 50%; width: 11%; aspect-ratio: 1; transform: translate(-50%, -50%); border-radius: 50%; display: flex; align-items: center; justify-content: center; background: var(--bg-raise); border: 1px solid var(--border); color: var(--fg); }
  .hub.warm { border-color: color-mix(in srgb, var(--accent) calc(30% + 60% * var(--heat)), var(--border)); }
  .diamond { position: absolute; left: calc(50% - 4px); top: calc(50% - 4px); width: 8px; height: 8px; box-sizing: border-box; border: 1.5px solid var(--accent); background: var(--accent-soft); }

  .host { position: absolute; width: calc(22% * var(--s)); aspect-ratio: 1; transform: translate(-50%, -50%); }
  .hostring { inset: 0; }
  .planet { position: absolute; inset: 30%; border-radius: 50%; display: flex; align-items: center; justify-content: center; background: radial-gradient(circle at 35% 30%, color-mix(in srgb, var(--accent) 30%, var(--bg-raise)), var(--bg-sunk)); border: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border)); color: var(--fg); }
  .planet .n { font-size: var(--text-2xs); font-weight: 600; color: var(--fg-2); }
  .host.busy .planet { border-color: color-mix(in srgb, var(--status-working) 70%, var(--border)); animation: breathe var(--loop-slow) ease-in-out infinite alternate; }
  /* The name and caption hang under the planet, never inside it: a long
     alias ellipsises against the stage, not against a 40 px circle. */
  .label { position: absolute; top: 72%; left: 50%; transform: translateX(-50%); width: max-content; max-width: 34cqw; display: flex; flex-direction: column; align-items: center; gap: 1px; text-align: center; pointer-events: none; }
  /* Moons (z-index 1) sit above the label, so a lower moon under the name stays clickable. */
  .name, .caption { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; pointer-events: auto; }
  .name { font-size: var(--text-xs); font-weight: 600; color: var(--fg); padding: 0 4px; border-radius: var(--radius-xs); background: color-mix(in srgb, var(--bg-sunk) 80%, transparent); }
  .caption { font-size: var(--text-2xs); }
  .ripple { position: absolute; inset: 30%; border-radius: 50%; border: 1.5px solid var(--accent); opacity: calc(0.25 + 0.75 * var(--heat)); pointer-events: none; animation: ripple calc(var(--loop-slow) * 2) ease-out infinite; }
  .hub .ripple { inset: 0; }
  .halo { position: absolute; inset: 26%; border-radius: 50%; border: 2px solid var(--status-waiting); pointer-events: none; animation: pulse var(--loop-slow) ease-in-out infinite; }
  .spark { position: absolute; right: 24%; top: 24%; width: 10px; height: 10px; border-radius: 50%; background: var(--status-failed); box-shadow: 0 0 8px 2px color-mix(in srgb, var(--status-failed) 60%, transparent); animation: pulse var(--loop-fast) ease-in-out infinite; }
  .comet { position: absolute; inset: 0; padding: 0; border: 0; border-radius: 50%; background: transparent; cursor: pointer; animation: orbit var(--lap) linear infinite; }
  .comet.quiet { opacity: 0.45; }
  .trail { position: absolute; inset: 0; border-radius: 50%; background: conic-gradient(from 0deg, transparent 0 calc(100% - var(--trail)), color-mix(in srgb, var(--status-working) 90%, transparent) 100%); mask: radial-gradient(farthest-side, transparent calc(100% - 3px), var(--bg) calc(100% - 2px)); -webkit-mask: radial-gradient(farthest-side, transparent calc(100% - 3px), var(--bg) calc(100% - 2px)); }
  .head { position: absolute; left: calc(50% - 4px); top: -3px; width: 8px; height: 8px; border-radius: 50%; background: var(--fg); box-shadow: 0 0 10px 3px var(--status-working); }
  /* Satellites: sessions at work, on a tighter orbit than the comets,
     spread round by --phase. */
  .sat { position: absolute; inset: 16%; border-radius: 50%; pointer-events: none; animation: orbit calc(var(--loop-slow) * 5) linear infinite; animation-delay: calc(var(--loop-slow) * -5 * var(--phase)); }
  .sathead { position: absolute; left: calc(50% - 3px); top: -3px; width: 6px; height: 6px; border-radius: 50%; background: var(--status-working); box-shadow: 0 0 6px 1px color-mix(in srgb, var(--status-working) 70%, transparent); }
  /* Moons: a 24 px target, drawn as an 8 px dot. */
  .moon { position: absolute; z-index: 1; width: 24px; height: 24px; transform: translate(-50%, -50%); padding: 0; border: 0; background: transparent; cursor: pointer; border-radius: 50%; }
  .moon::after { content: ''; position: absolute; left: 8px; top: 8px; width: 8px; height: 8px; border-radius: 50%; background: var(--fg-muted); box-shadow: 0 0 0 2px var(--bg-sunk); }
  .moon.done::after { background: var(--status-done); }
  .moon.failed::after { background: var(--status-failed); }
  .moon.waiting::after { background: var(--status-waiting); }
  .moon.jev::after { background: var(--accent); }
  .moon:hover::after { transform: scale(1.4); }

  .side { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
  .card { background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); padding: 12px 14px; display: flex; flex-direction: column; gap: 8px; }
  .legend { display: grid; grid-template-columns: 18px minmax(0, 1fr); gap: 8px 10px; align-items: center; }
  .lg { justify-self: center; display: inline-block; }
  .planet-sw { width: 14px; height: 14px; border-radius: 50%; background: var(--bg-raise); border: 1px solid var(--border); }
  .comet-sw { width: 8px; height: 8px; border-radius: 50%; background: var(--status-working); box-shadow: 0 0 6px 2px color-mix(in srgb, var(--status-working) 60%, transparent); }
  .sat-sw { width: 6px; height: 6px; border-radius: 50%; background: var(--status-working); }
  .moon-sw { width: 8px; height: 8px; border-radius: 50%; background: var(--status-done); }
  .ripple-sw { width: 14px; height: 14px; border-radius: 50%; border: 1.5px solid var(--accent); }
  .halo-sw { width: 10px; height: 10px; border-radius: 50%; border: 2px solid var(--status-waiting); }
  .spark-sw { width: 8px; height: 8px; border-radius: 50%; background: var(--status-failed); }
  .dia-sw { width: 8px; height: 8px; transform: rotate(45deg); border: 1.5px solid var(--accent); }
  .tick-sw { width: 2px; height: 12px; background: var(--fg-muted); }
  .feed-head { display: flex; align-items: center; gap: 6px; }
  .feed-head .lbl { flex: 1; }
  .chip { height: 20px; padding: 0 7px; gap: 5px; border-radius: var(--radius-xs); font-size: var(--text-2xs); display: inline-flex; align-items: center; background: var(--bg-raise); color: var(--fg-2); overflow: hidden; }
  .chip.on, .chip.working { color: var(--fg); }
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
  @keyframes ripple { from { transform: scale(1); } to { transform: scale(2.2); opacity: 0; } }
  @keyframes flow { to { stroke-dashoffset: -14; } }
  @keyframes breathe { to { box-shadow: 0 0 14px 2px color-mix(in srgb, var(--status-working) 45%, transparent); } }
  @keyframes beacon { to { box-shadow: 0 0 0 8px transparent; } }
  /* Reduced motion: nothing travels; the halo, spark, ripple and beacon
     fade once a beat. Off holds everything still (motion.ts sets data-motion). */
  :global(:root[data-motion='reduced']) .comet,
  :global(:root[data-motion='reduced']) .sat,
  :global(:root[data-motion='reduced']) .spoke,
  :global(:root[data-motion='reduced']) .planet { animation: none; }
  :global(:root[data-motion='reduced']) .halo,
  :global(:root[data-motion='reduced']) .spark,
  :global(:root[data-motion='reduced']) .ripple,
  :global(:root[data-motion='reduced']) .nowmark { animation: motion-fade var(--loader-reduced) ease-in-out infinite; }
  :global(:root[data-motion='off']) .comet,
  :global(:root[data-motion='off']) .sat,
  :global(:root[data-motion='off']) .spoke,
  :global(:root[data-motion='off']) .planet,
  :global(:root[data-motion='off']) .halo,
  :global(:root[data-motion='off']) .spark,
  :global(:root[data-motion='off']) .ripple,
  :global(:root[data-motion='off']) .nowmark { animation: none; }
  .orbit.compact { grid-template-columns: minmax(0, 1fr); gap: 12px; }
  @media (max-width: 900px) { .orbit { grid-template-columns: minmax(0, 1fr); } }
</style>
