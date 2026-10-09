<!--
  Automation (Orbit Fleet redesign step 8.4, board Automation): the New
  layout's rail item for what runs on the fleet's behalf, read-only.
  Routines lists the built-in ones, fleet's own loops (8.1), with when each
  last ran and runs next; Runs lists every run (8.3), each linked to its
  session; Agents names the three agents fleet runs itself. The head says
  today's spend and holds Pause all (`automation.paused`). Routines a person
  writes (8.6) join the Routines tab above the built-in ones.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Tabs from './kit/Tabs.svelte';
  import Button from './kit/Button.svelte';
  import Loader from './Loader.svelte';
  import { fleetSettings, loadFleetSettings } from './fleet_settings';
  import { listRuns, outcomeLabel, type RunRow } from './runs';
  import { goTo } from './destination';
  import { focusSession } from './session_focus';
  import { pushError } from './toasts';
  import { timeAgo } from './session_status';
  import {
    automationTab,
    builtInAgents,
    loadAutomation,
    loopEvery,
    loopLine,
    money,
    setAutomationPaused,
    spendMicros,
    type AutomationState,
    type AutomationTab,
  } from './automation';

  let auto = $state<AutomationState | null>(null);
  let error = $state<string | null>(null);
  let runs = $state<RunRow[] | null>(null);
  let runsTotal = $state(0);
  let busy = $state(false);
  let nowSec = $state(Math.floor(Date.now() / 1000));

  async function load() {
    const r = await loadAutomation();
    if (r.ok) {
      auto = r.value;
      error = null;
    } else error = r.error.message;
  }

  async function loadRuns() {
    const r = await listRuns({ limit: 50 });
    if (r.ok) {
      runs = r.value.runs;
      runsTotal = r.value.total;
    } else error = r.error.message;
  }

  onMount(() => {
    void loadFleetSettings();
    void load();
    void loadRuns();
    // The loops' "next in" and "ago" move; the data itself is re-read on the
    // same beat, as a loop's last run changes without an event.
    const t = setInterval(() => {
      nowSec = Math.floor(Date.now() / 1000);
      void load();
    }, 30_000);
    return () => clearInterval(t);
  });

  const paused = $derived(auto?.paused ?? false);
  const spend = $derived(auto ? spendMicros(auto.today) : null);
  const running = $derived(auto?.today.filter((r) => r.outcome === 'running').length ?? 0);
  const agents = $derived(auto ? builtInAgents($fleetSettings, auto.loops, auto.today, nowSec) : []);

  async function togglePause() {
    busy = true;
    const r = await setAutomationPaused(!paused);
    busy = false;
    if (!r.ok) {
      pushError(r.error, paused ? 'Resume failed' : 'Pause all failed');
      return;
    }
    await load();
  }

  function openRun(run: RunRow) {
    const id = run.session_ids[0];
    if (id === undefined) return;
    if (focusSession(id, run.owner)) goTo('session');
  }

  function duration(run: RunRow): string {
    const ms = run.duration_ms ?? (run.ended_at ? (run.ended_at - run.started_at) * 1000 : null);
    if (ms === null) return '';
    const s = Math.round(ms / 1000);
    return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
  }
</script>

<section class="automation" aria-label="Automation" data-testid="automation-view">
  <header class="head">
    <h1>Automation</h1>
    <Tabs
      label="Automation"
      testid="automation-tabs"
      selected={$automationTab}
      onselect={(id) => automationTab.set(id as AutomationTab)}
      tabs={[
        { id: 'routines', label: 'Routines', count: auto?.loops.length },
        { id: 'runs', label: 'Runs', count: runs ? runsTotal : undefined },
        { id: 'agents', label: 'Agents', count: 3 },
      ]}
    />
    <span class="grow"></span>
    {#if spend !== null}
      <span class="today" data-testid="automation-today">Today {money(spend)}{#if running > 0}{' '}· {running} running{/if}</span>
    {/if}
    <Button testid="automation-pause" disabled={busy || !auto} onclick={togglePause}>
      {paused ? 'Resume' : 'Pause all'}
    </Button>
  </header>

  {#if paused}
    <p class="banner" role="status" data-testid="automation-paused">
      Paused: missions, garbage collection, syncs, playbooks and repairs stand still until you resume. Reconcile, usage and
      update checks keep running.
    </p>
  {/if}
  {#if error}<p class="err" role="alert" data-testid="automation-error">{error}</p>{/if}

  {#if !auto && !error}
    <div class="loading"><Loader name="orbit" size={32} label="Loading automation" /></div>
  {:else if $automationTab === 'runs'}
    <div class="body" role="tabpanel" aria-label="Runs" data-testid="automation-runs">
      {#if runs && runs.length === 0}
        <p class="none">Nothing has run on the fleet's behalf yet.</p>
      {:else if runs}
        <ul class="list">
          {#each runs as run (run.id)}
            <li class="row" data-testid="automation-run" data-outcome={run.outcome}>
              <span class="when">{timeAgo(run.started_at, nowSec * 1000)}</span>
              <span class="main">
                <strong>{run.owner}</strong>
                <span class="meta">{run.kind}{run.host ? ` · ${run.host}` : ''}</span>
                <span class={`outcome ${run.outcome}`}>{outcomeLabel(run)}</span>
                {#if run.summary}<span class="summary">{run.summary}</span>{/if}
              </span>
              <span class="num">{duration(run)}</span>
              <span class="num">{run.cost_micros !== undefined ? money(run.cost_micros) : ''}</span>
              {#if run.session_ids.length > 0}
                <Button variant="quiet" size="sm" testid="automation-run-session" onclick={() => openRun(run)}>Session</Button>
              {/if}
            </li>
          {/each}
        </ul>
        {#if runsTotal > runs.length}<p class="none">Showing the latest {runs.length} of {runsTotal}.</p>{/if}
      {/if}
    </div>
  {:else if $automationTab === 'agents'}
    <div class="body" role="tabpanel" aria-label="Agents" data-testid="automation-agents">
      <p class="hint">Built in: the agents fleet runs itself.</p>
      <ul class="list">
        {#each agents as a (a.id)}
          <li class="row" data-testid="automation-agent-{a.id}">
            <span class="main">
              <strong>{a.name}</strong>
              <span class="summary">{a.does}</span>
            </span>
            <span class="meta state">{a.state}</span>
          </li>
        {/each}
      </ul>
    </div>
  {:else if auto}
    <div class="body" role="tabpanel" aria-label="Routines" data-testid="automation-routines">
      <h2>Built in <span class="meta">{auto.loops.length}</span></h2>
      <p class="hint">Fleet's own loops. Pause all stops the ones that act on their own; the rest only observe.</p>
      {#if auto.loops.length === 0}
        <p class="none">This fleet does not report its loops yet.</p>
      {/if}
      <ul class="list">
        {#each auto.loops as loop (loop.name)}
          <li class="row" data-testid="automation-loop" data-loop={loop.name} data-result={loop.result ?? 'none'}>
            <span class="main">
              <strong>{loop.label}</strong>
              <span class="meta">System · {loop.pausable ? 'acts on its own' : 'observes'}</span>
              <span class="summary" class:failed={loop.result === 'error'}>{loopLine(loop, nowSec, paused)}</span>
            </span>
            <span class="num">{loopEvery(loop) ?? ''}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>

<style>
  .automation { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .head { display: flex; align-items: center; gap: var(--space-3, 12px); padding: var(--space-3, 12px) var(--space-4, 16px) 0; border-bottom: 1px solid var(--border); }
  h1 { margin: 0; font-size: var(--text-lg, 15px); }
  h2 { margin: 0; font-size: var(--text-md, 13px); }
  .grow { flex: 1; }
  .today { font-size: var(--text-sm, 12.5px); color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .banner { margin: 0; padding: var(--space-2, 8px) var(--space-4, 16px); font-size: var(--text-sm, 12.5px); background: color-mix(in srgb, var(--status-waiting) 14%, transparent); }
  .err { margin: 0; padding: var(--space-2, 8px) var(--space-4, 16px); font-size: var(--text-sm, 12.5px); color: var(--status-failed); }
  .loading { display: flex; justify-content: center; padding: var(--space-6, 24px); }
  .body { flex: 1; min-height: 0; overflow: auto; padding: var(--space-3, 12px) var(--space-4, 16px); display: flex; flex-direction: column; gap: var(--space-2, 8px); }
  .hint, .none { margin: 0; font-size: var(--text-sm, 12.5px); color: var(--fg-muted); }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .row { display: flex; align-items: center; gap: var(--space-3, 12px); padding: var(--space-2, 8px) 0; border-bottom: 1px solid var(--border); }
  .main { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; }
  .summary { flex-basis: 100%; font-size: var(--text-sm, 12.5px); color: var(--fg-muted); overflow-wrap: anywhere; }
  .summary.failed, .outcome.failed { color: var(--status-failed); }
  .outcome { font-size: var(--text-sm, 12.5px); }
  .outcome.needs_person { color: var(--status-waiting); }
  .meta, .when, .num { font-size: var(--text-xs, 11.5px); color: var(--fg-muted); }
  .when { width: 5.5em; flex: none; }
  .num { font-variant-numeric: tabular-nums; flex: none; }
  .state { flex: none; }
</style>
