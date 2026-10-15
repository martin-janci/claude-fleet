<!--
  One run's timeline (automation runs board A): what it is and how it is
  doing at the top, its phases from trigger to outcome as a rail, every
  step it recorded on the run's clock, and the picked step's data, with
  the checks that let it run. Reads `run_trace` and reads on at each
  `run:changed` for this run, so a live run grows while it is watched.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import Button from '../kit/Button.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import type { OfState } from '../kit/status';
  import LoadError from '../states/LoadError.svelte';
  import type { IpcError } from '../result';
  import { RUN_CHANGED_EVENT } from '../events';
  import { money } from '../automation';
  import { copyText } from '../clipboard';
  import {
    hasTrace,
    outcomeLabel,
    phaseRail,
    readTraceOn,
    stepBar,
    tookWords,
    type PhaseState,
    type RunRow,
    type RunTraceStep,
  } from '../runs';

  let {
    run,
    nowSec,
    onclose,
    onopensession,
  }: {
    run: RunRow;
    nowSec: number;
    onclose?: () => void;
    onopensession?: (run: RunRow) => void;
  } = $props();

  let steps = $state<RunTraceStep[]>([]);
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  let picked = $state<number | null>(null);
  let reading = false;
  let again = false;

  /** Read the steps after the last one held; a read asked for while one
   *  runs is done once it ends, so no event is lost. */
  async function readOn() {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    do {
      again = false;
      const r = await readTraceOn(run.id, steps);
      if (r.ok) {
        steps = r.value;
        error = null;
      } else if (steps.length === 0) error = r.error;
    } while (again);
    reading = false;
    loaded = true;
  }

  onMount(() => {
    if (!hasTrace(run)) {
      loaded = true;
      return;
    }
    void readOn();
    let un: (() => void) | null = null;
    let gone = false;
    void listen<{ run_ref?: string }>(RUN_CHANGED_EVENT, (e) => {
      if (e.payload?.run_ref === run.id) void readOn();
    }).then((u) => (gone ? u() : (un = u)));
    return () => {
      gone = true;
      un?.();
    };
  });

  const live = $derived(run.outcome === 'running');
  const rail = $derived(phaseRail(steps, run.outcome));
  const from = $derived(Math.min(run.started_at, steps[0]?.at ?? run.started_at));
  const to = $derived(
    live ? Math.max(nowSec, from + 1) : Math.max(run.ended_at ?? 0, steps[steps.length - 1]?.at ?? 0, from + 1),
  );
  const elapsed = $derived(Math.max(0, (live ? nowSec : (run.ended_at ?? to)) - run.started_at));
  const cost = $derived(
    run.cost_micros ?? steps.reduce<number | undefined>((a, s) => (s.cost_micros !== undefined ? s.cost_micros : a), undefined),
  );
  const pick = $derived(steps.find((s) => s.seq === picked) ?? steps.findLast((s) => s.phase === 'working') ?? steps[steps.length - 1]);
  const checks = $derived.by(() => {
    const step = steps.find((s) => s.phase === 'checks');
    const list = step?.data?.checks;
    return Array.isArray(list) ? (list as { name?: string; state?: string; detail?: string }[]) : [];
  });
  const RUN_DOT: Record<string, OfState> = { ok: 'done', nothing_to_do: 'idle', failed: 'failed', needs_person: 'waiting', running: 'working' };
  const RAIL_DOT: Record<PhaseState, string> = { done: 'done', live: 'live', failed: 'failed', skipped: 'skipped', todo: 'todo' };
  const clock = (sec: number) => {
    const d = new Date(sec * 1000);
    return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
  };
  const mmss = (s: number) => {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const ss = String(s % 60).padStart(2, '0');
    return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${String(m).padStart(2, '0')}:${ss}`;
  };
  const number = $derived(run.id.split(':')[1] ?? run.id);
  const stepTone = (s: RunTraceStep) =>
    s.kind === 'error' ? 'failed' : s.kind === 'decision' || s.actor === 'jev' ? 'decision' : s.phase === 'working' ? 'work' : 'phase';

  async function copyStep() {
    if (pick) await copyText(JSON.stringify(pick, null, 2));
  }
</script>

<section class="timeline" aria-label={`Run ${run.owner}`} data-testid="run-timeline" data-live={live}>
  <header class="head">
    {#if live}<span class="comet" aria-hidden="true"></span>{:else}<StatusDot state={RUN_DOT[run.outcome] ?? 'idle'} label={null} />{/if}
    <div class="title">
      <div class="line">
        <strong class="name">{run.owner}</strong>
        <span class={`chip ${run.outcome}`} data-testid="run-timeline-outcome">{outcomeLabel(run)}</span>
        <span class="muted xs">{run.kind} · run #{number}</span>
      </div>
      <span class="xs fg2">
        Started {clock(run.started_at)}{run.host ? ` · on ${run.host}` : ''}{#if live && run.summary}{' '}· now <span class="mono">{run.summary}</span><span class="blink" aria-hidden="true">▍</span>{/if}
      </span>
    </div>
    <div class="fig"><span class="big tnum" data-testid="run-timeline-elapsed">{mmss(elapsed)}</span><span class="xs muted">{live ? 'running' : 'took'}</span></div>
    {#if cost !== undefined}
      <span class="sep"></span>
      <div class="fig"><span class="big tnum">{money(cost)}</span><span class="xs muted">spent</span></div>
    {/if}
    {#if run.session_ids.length > 0 && onopensession}
      <Button testid="run-timeline-session" onclick={() => onopensession?.(run)}>Open session</Button>
    {/if}
    {#if onclose}<Button variant="quiet" testid="run-timeline-close" onclick={() => onclose?.()}>Back to runs</Button>{/if}
  </header>

  {#if !hasTrace(run)}
    <p class="none" data-testid="run-timeline-untraced">
      This kind of run keeps no step trace yet. Routine fires and agent jobs do.{run.summary ? ` ${run.summary}` : ''}
    </p>
  {:else if error}
    <LoadError title="Couldn't load the run's steps" {error} onretry={() => void readOn()} testid="run-timeline-error" />
  {:else}
    <div class="phases" data-testid="run-timeline-phases">
      <span class="lbl">Phases</span>
      <ol class="rail">
        {#each rail as p, i (p.phase)}
          <li class="cell" data-state={p.state} data-testid={`run-phase-${p.phase}`}>
            <span class={`pdot ${RAIL_DOT[p.state]}`}></span>
            <span class="xs pname">{p.label}</span>
            <span class="xs muted pmeta" title={p.meta}>{p.meta}{p.ms ? ` · ${tookWords(p.ms)}` : ''}</span>
            {#if i < rail.length - 1}<span class={`track ${p.state === 'done' || p.state === 'skipped' ? 'done' : ''} ${p.state === 'live' ? 'live' : ''}`}></span>{/if}
          </li>
        {/each}
      </ol>
    </div>

    <div class="grid">
      <div class="pane steps">
        <div class="pane-head">
          <strong>What it did</strong>
          <span class="xs muted">{live ? 'every step, live' : 'every step'} · pick one to see its data</span>
        </div>
        <div class="scale xs muted tnum" aria-hidden="true">
          <span></span><span></span>
          <span class="ticks"><span>{clock(from)}</span><span>{live ? 'now' : clock(to)}</span></span>
          <span></span>
        </div>
        {#if loaded && steps.length === 0}
          <p class="none">No steps recorded. A run from before traces, or a hub that keeps none, shows nothing here.</p>
        {/if}
        <ul class="list" role="listbox" aria-label="Steps">
          {#each steps as s (s.seq)}
            {@const bar = stepBar(s, from, to)}
            <li>
              <button
                type="button"
                role="option"
                class="row"
                aria-selected={pick?.seq === s.seq}
                data-testid="run-step"
                data-kind={s.kind}
                onclick={() => (picked = s.seq)}
              >
                <span class={`sdot ${stepTone(s)}`}></span>
                <span class="stext">
                  <span class="xs stitle">{s.summary}</span>
                  <span class="mono muted ssub">{s.phase} · {s.actor}{s.attempt > 1 ? ` · try ${s.attempt}` : ''}</span>
                </span>
                <span class="sbar"><span class={`bar ${stepTone(s)}`} style:left={`${bar.left}%`} style:width={`${bar.width}%`}></span></span>
                <span class="xs tnum fg2 took">{tookWords(s.duration_ms)}</span>
              </button>
            </li>
          {/each}
          {#if live}
            <li class="row ghost" aria-hidden="true">
              <span class="sdot live"></span>
              <span class="xs muted">{run.summary ?? 'Working'}</span>
              <span class="sbar"><span class="bar live" style:left="96%" style:width="4%"></span></span>
              <span></span>
            </li>
          {/if}
        </ul>
      </div>

      <div class="side">
        <div class="pane detail" data-testid="run-step-detail">
          <div class="pane-head"><strong>Step detail</strong>{#if pick}<span class="chip">{pick.kind}</span>{/if}</div>
          {#if pick}
            <span class="dtitle">{pick.summary}</span>
            <span class="xs fg2">{clock(pick.at)} · {pick.actor}{pick.duration_ms !== undefined ? ` · took ${tookWords(pick.duration_ms)}` : ''}{pick.decision_id ? ` · Jev decision ${pick.decision_id}` : ''}</span>
            <pre class="mono raw">{JSON.stringify(pick, null, 2)}</pre>
            <div><Button size="sm" testid="run-step-copy" onclick={copyStep}>Copy as JSON</Button></div>
          {:else}
            <span class="xs muted">No step yet.</span>
          {/if}
        </div>
        {#if checks.length > 0}
          <div class="pane checks" data-testid="run-checks">
            <div class="pane-head"><strong>Why it ran</strong><span class="xs muted">{checks.length} checks</span></div>
            {#each checks as c, i (i)}
              <div class="check">
                <span class={`mark ${c.state ?? ''}`}>{c.state === 'block' ? '✗' : c.state === 'warn' ? '!' : '✓'}</span>
                <span class="xs cname">{c.name}</span>
                <span class="xs fg2">{c.detail}</span>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</section>

<style>
  .timeline { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: auto; }
  .head { display: flex; align-items: center; gap: 14px; padding: 12px var(--space-6); border-bottom: 1px solid var(--border); background: var(--bg-pane); }
  .title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .line { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .name { font-size: var(--text-md); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .fig { display: flex; flex-direction: column; align-items: flex-end; gap: 2px; }
  .big { font-size: var(--text-lg); font-weight: 600; }
  .sep { width: 1px; height: 28px; background: var(--border); }
  .xs { font-size: var(--text-xs); }
  .muted { color: var(--fg-muted); }
  .fg2 { color: var(--fg-2); }
  .tnum { font-variant-numeric: tabular-nums; }
  .mono { font-family: var(--font-mono); font-size: var(--text-code); }
  .lbl { font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.06em; }
  .chip { height: 20px; padding: 0 7px; border-radius: var(--radius-xs); font-size: var(--text-2xs); font-weight: 500; display: inline-flex; align-items: center; background: var(--bg-raise); color: var(--fg-2); white-space: nowrap; }
  .chip.running { background: color-mix(in srgb, var(--status-working) 14%, transparent); color: var(--status-working); }
  .chip.ok { background: color-mix(in srgb, var(--status-done) 14%, transparent); color: var(--status-done); }
  .chip.failed { background: color-mix(in srgb, var(--status-failed) 14%, transparent); color: var(--status-failed); }
  .chip.needs_person { background: color-mix(in srgb, var(--status-waiting) 14%, transparent); color: var(--status-waiting); }
  .comet { width: 12px; height: 12px; border-radius: 50%; background: var(--status-working); flex: none; animation: halo var(--loop-slow) ease-out infinite; }
  .blink { animation: blink var(--loop-fast) steps(2) infinite; }
  .none { margin: var(--space-4) var(--space-6); font-size: var(--text-sm); color: var(--fg-muted); }

  .phases { padding: 16px var(--space-6) 12px; display: flex; flex-direction: column; gap: 10px; }
  .rail { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(8, minmax(0, 1fr)); }
  .cell { position: relative; display: flex; flex-direction: column; align-items: center; gap: 5px; min-width: 0; padding: 0 4px; text-align: center; }
  .pname { font-weight: 500; }
  .pmeta { max-width: 100%; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .pdot { width: 8px; height: 8px; border-radius: 50%; background: var(--border); position: relative; z-index: 1; }
  .pdot.done { background: var(--status-done); }
  .pdot.failed { background: var(--status-failed); }
  .pdot.skipped { background: transparent; border: 1.5px solid var(--fg-muted); box-sizing: border-box; }
  .pdot.live { width: 12px; height: 12px; margin-top: -2px; background: var(--status-working); animation: halo var(--loop-slow) ease-out infinite; }
  .track { position: absolute; top: 3px; left: calc(50% + 8px); right: calc(-50% + 8px); height: 2px; background: var(--border); overflow: hidden; }
  .track.done { background: var(--status-done); }
  .track.live::after { content: ''; position: absolute; top: 0; left: -40%; width: 40%; height: 2px; background: linear-gradient(90deg, transparent, var(--status-working)); animation: fly var(--loop-slow) linear infinite; }

  .grid { flex: 1; min-height: 0; padding: 0 var(--space-6) var(--space-4); display: grid; grid-template-columns: minmax(0, 1fr) minmax(280px, 400px); gap: 16px; }
  .pane { background: var(--bg-pane); border: 1px solid var(--border); border-radius: var(--radius-md); }
  .pane-head { display: flex; align-items: center; gap: 10px; }
  .pane-head strong { flex: 1; }
  .steps { display: flex; flex-direction: column; min-height: 0; padding: 12px 4px 8px; gap: 6px; }
  .steps .pane-head { padding: 0 10px; }
  .steps .pane-head strong { flex: none; }
  .scale, .row { display: grid; grid-template-columns: 14px minmax(140px, 230px) minmax(0, 1fr) 54px; gap: 10px; align-items: center; }
  .scale { padding: 0 10px; }
  .ticks { display: flex; justify-content: space-between; }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 1px; overflow: auto; }
  .row { width: 100%; appearance: none; border: 0; background: transparent; color: inherit; font: inherit; text-align: left; padding: 5px 10px; border-radius: var(--radius-sm); cursor: pointer; }
  .row:hover { background: var(--bg-hover); }
  .row[aria-selected='true'] { background: var(--accent-soft); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent); }
  .row.ghost { cursor: default; }
  .stext { display: flex; flex-direction: column; min-width: 0; }
  .stitle { font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .ssub { font-size: var(--text-2xs); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .took { text-align: right; }
  .sdot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg-muted); }
  .sdot.work { background: var(--status-working); opacity: 0.75; }
  .sdot.phase { background: var(--status-done); }
  .sdot.decision { background: var(--status-waiting); }
  .sdot.failed { background: var(--status-failed); }
  .sdot.live { background: var(--status-working); animation: halo var(--loop-slow) ease-out infinite; }
  .sbar { position: relative; height: 6px; border-radius: var(--radius-xs); background: color-mix(in srgb, var(--border) 60%, transparent); }
  .bar { position: absolute; top: 0; height: 6px; border-radius: var(--radius-xs); background: var(--fg-muted); }
  .bar.work { background: color-mix(in srgb, var(--status-working) 60%, var(--bg-raise)); }
  .bar.phase { background: var(--status-done); }
  .bar.decision { background: var(--status-waiting); }
  .bar.failed { background: var(--status-failed); }
  .bar.live { background: repeating-linear-gradient(90deg, var(--status-working) 0 14px, color-mix(in srgb, var(--status-working) 25%, transparent) 14px 24px); background-size: 24px 6px; animation: stripes var(--loop-fast) linear infinite; }

  .side { display: flex; flex-direction: column; gap: 12px; min-height: 0; }
  .detail, .checks { padding: 12px 14px; display: flex; flex-direction: column; gap: 8px; }
  .dtitle { font-size: var(--text-md); font-weight: 500; overflow-wrap: anywhere; }
  .raw { margin: 0; padding: 10px; background: var(--bg-sunk); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--fg-2); white-space: pre-wrap; overflow-wrap: anywhere; max-height: 260px; overflow: auto; }
  .check { display: grid; grid-template-columns: 14px 96px minmax(0, 1fr); gap: 8px; align-items: baseline; }
  .cname { font-weight: 500; }
  .mark { color: var(--status-done); font-weight: 600; }
  .mark.warn { color: var(--status-waiting); }
  .mark.block { color: var(--status-failed); }

  @keyframes halo { 0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--status-working) 55%, transparent); } 100% { box-shadow: 0 0 0 10px transparent; } }
  @keyframes fly { to { left: 100%; } }
  @keyframes stripes { to { background-position: 24px 0; } }
  @keyframes blink { 50% { opacity: 0; } }
  /* Reduced motion makes each loop the one motion-fade; Off holds it
     still (motion.ts sets data-motion, the OS preference included). */
  :global(:root[data-motion='reduced']) .comet,
  :global(:root[data-motion='reduced']) .pdot.live,
  :global(:root[data-motion='reduced']) .sdot.live,
  :global(:root[data-motion='reduced']) .bar.live,
  :global(:root[data-motion='reduced']) .blink { animation: motion-fade var(--loader-reduced) ease-in-out infinite; }
  :global(:root[data-motion='reduced']) .track.live::after { animation: none; }
  :global(:root[data-motion='off']) .comet,
  :global(:root[data-motion='off']) .pdot.live,
  :global(:root[data-motion='off']) .sdot.live,
  :global(:root[data-motion='off']) .track.live::after,
  :global(:root[data-motion='off']) .bar.live,
  :global(:root[data-motion='off']) .blink { animation: none; }
</style>
