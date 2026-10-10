<!--
  Automation (Orbit Fleet redesign step 8.4, board Automation): the New
  layout's rail item for what runs on the fleet's behalf. Its own left
  column holds the title with + New, the Routines · Runs · Agents · Rules
  switch, and today's spend with Pause all (`automation.paused`) at its
  foot. Routines (RoutinesPanel) lists the ones a person writes (8.6), then
  the built-in ones, fleet's own loops (8.1); Runs lists every run (8.3),
  each linked to its session; Agents names the three agents fleet runs
  itself; Rules holds the start rules (8.11, StartRules).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import { tablistKeys } from './tablist_keys';
  import Count from './kit/Count.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import type { OfState } from './kit/status';
  import NewRoutineMenu from './automation/NewRoutineMenu.svelte';
  import { openRoutines } from './routines';
  import Loader from './Loader.svelte';
  import RoutinesPanel from './automation/RoutinesPanel.svelte';
  import StartRules from './StartRules.svelte';
  import { fleetSettings, loadFleetSettings } from './fleet_settings';
  import { listRuns, outcomeLabel, type RunOutcome, type RunRow } from './runs';
  import { goTo } from './destination';
  import { focusSession } from './session_focus';
  import { pushError } from './toasts';
  import { errorText } from './error_copy';
  import type { IpcError } from './result';
  import LoadError from './states/LoadError.svelte';
  import { timeAgo } from './session_status';
  import {
    automationTab,
    builtInAgents,
    loadAutomation,
    money,
    setAutomationPaused,
    spendMicros,
    type AutomationState,
    type AutomationTab,
  } from './automation';
  import { windowHidden } from './window_hidden';
  import { routineBudget, type FleetBudget } from './routines';

  let auto = $state<AutomationState | null>(null);
  /** The automation read failed (review r13: said as a failure, with Retry). */
  let error = $state<IpcError | null>(null);
  let runs = $state<RunRow[] | null>(null);
  /** The Runs read has its own error, so a later good `load()` cannot clear
   *  it and leave the tab blank. */
  let runsError = $state<IpcError | null>(null);
  let retrying = $state(false);
  let runsTotal = $state(0);
  /** The Runs tab's outcome filter, from its left column; null shows all. */
  let runsOutcome = $state<RunOutcome | null>(null);
  let busy = $state(false);
  let nowSec = $state(Math.floor(Date.now() / 1000));

  /** The routines' spend today against `automation.daily_budget` (G3.8);
   *  null from an older hub, which has no `budget` action. */
  let fleetBudget = $state<FleetBudget | null>(null);

  async function load() {
    void routineBudget().then((b) => (fleetBudget = b.ok ? b.value : null));
    const r = await loadAutomation();
    if (r.ok) {
      auto = r.value;
      error = null;
    } else error = r.error;
  }

  async function loadRuns() {
    const r = await listRuns({ limit: 50, ...(runsOutcome ? { outcome: runsOutcome } : {}) });
    if (r.ok) {
      runs = r.value.runs;
      runsTotal = r.value.total;
      runsError = null;
    } else runsError = r.error;
  }

  async function retry(what: () => Promise<void>) {
    retrying = true;
    await what();
    retrying = false;
  }

  onMount(() => {
    void loadFleetSettings();
    void load();
    void loadRuns();
    // The loops' "next in" and "ago" move; the data itself is re-read on the
    // same beat, as a loop's last run changes without an event.
    const t = setInterval(() => {
      if (windowHidden()) return;
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

  const RUN_DOT: Record<string, OfState> = { ok: 'done', nothing_to_do: 'idle', failed: 'failed', needs_person: 'waiting', running: 'working' };

  const OUTCOMES: { id: RunOutcome | null; label: string; state: OfState }[] = [
    { id: null, label: 'All runs', state: 'idle' },
    { id: 'failed', label: 'Failed', state: 'failed' },
    { id: 'needs_person', label: 'Needs you', state: 'waiting' },
    { id: 'running', label: 'Working', state: 'working' },
    { id: 'ok', label: 'Done', state: 'done' },
    { id: 'nothing_to_do', label: 'Nothing to do', state: 'idle' },
  ];

  function showOutcome(o: RunOutcome | null) {
    runsOutcome = o;
    runs = null;
    void loadRuns();
  }

  const TABS: { id: AutomationTab; label: string; count?: () => number | undefined; title?: string }[] = [
    { id: 'routines', label: 'Routines' },
    { id: 'runs', label: 'Runs' },
    { id: 'agents', label: 'Agents', count: () => 3, title: 'The agents fleet runs itself' },
    { id: 'rules', label: 'Rules', title: 'Start rules: what starts on its own' },
  ];

  function duration(run: RunRow): string {
    const ms = run.duration_ms ?? (run.ended_at ? (run.ended_at - run.started_at) * 1000 : null);
    if (ms === null) return '';
    const s = Math.round(ms / 1000);
    return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
  }
</script>

<!-- Automation's own left column (UX audit 2026-10-09; board Automation):
     its title, the tabs, Routines' list (yours, then built in), and today's
     spend with Pause all at its foot. The detail fills the rest. -->
{#snippet sideHead()}
  <div class="side-head">
    <div class="title-row">
      <h1>Automation</h1>
      <NewRoutineMenu onpick={(t) => openRoutines({ template: t })} />
    </div>
    <div class="seg" role="tablist" aria-label="Automation" data-testid="automation-tabs" use:tablistKeys>
      {#each TABS as t (t.id)}
        {@const n = t.id === 'runs' ? (runs ? runsTotal : undefined) : t.count?.()}
        <button
          type="button"
          role="tab"
          aria-selected={$automationTab === t.id}
          title={t.title}
          data-testid={`automation-tab-${t.id}`}
          onclick={() => automationTab.set(t.id)}>{t.label}{#if n !== undefined}{' '}<Count {n} />{/if}</button
        >
      {/each}
    </div>
  </div>
{/snippet}

{#snippet sideFoot()}
  <footer class="side-foot">
    {#if spend !== null}
      <span class="today" data-testid="automation-today">Today {money(spend)}{#if running > 0}{' '}· {running} running{/if}</span>
    {/if}
    {#if fleetBudget?.budget_micros}
      <span
        class="today"
        class:over={fleetBudget.spent_micros >= fleetBudget.budget_micros}
        data-testid="automation-budget"
        title="Every routine's runs in this UTC day, against the routines daily budget (automation.daily_budget). Once spent, no routine starts a run until tomorrow."
        >Routines {money(fleetBudget.spent_micros)} of {money(fleetBudget.budget_micros)} budget</span
      >
    {/if}
    <span class="grow"></span>
    <Button variant={paused ? 'default' : 'danger'} testid="automation-pause" disabled={busy || !auto} onclick={togglePause}>
      {paused ? 'Resume' : 'Pause all'}
    </Button>
  </footer>
{/snippet}

<section class="automation" aria-label="Automation" data-testid="automation-view">
  {#if paused}
    <p class="banner" role="status" data-testid="automation-paused">
      Paused: missions, garbage collection, syncs, playbooks and repairs stand still until you resume. Reconcile, usage and
      update checks keep running.
    </p>
  {/if}
  {#if error && auto}
    <!-- A refresh failed: what is shown is the last good read. -->
    <p class="err" role="alert" data-testid="automation-error">
      Couldn't refresh automation: {errorText(error)}. Showing what was read before.
      <Button variant="quiet" size="sm" testid="automation-retry" disabled={retrying} onclick={() => retry(load)}>Retry</Button>
    </p>
  {/if}

  {#if $automationTab === 'routines' && auto}
    <div class="fill" role="tabpanel" aria-label="Routines" data-testid="automation-routines-yours">
      <RoutinesPanel fill listHead={sideHead} listFoot={sideFoot} loops={auto.loops} {paused} {nowSec} />
    </div>
  {:else}
    <div class="split">
      <aside class="side">
        {@render sideHead()}
        {#if $automationTab === 'runs'}
          <ul class="outcomes" role="listbox" aria-label="Show runs" data-testid="automation-runs-filter">
            <li class="of-sec" role="presentation">Show</li>
            {#each OUTCOMES as o (o.label)}
              <li>
                <button type="button" role="option" class="of-row" aria-selected={runsOutcome === o.id} data-testid={`automation-runs-${o.id ?? 'all'}`} onclick={() => showOutcome(o.id)}>
                  {#if o.id}<StatusDot state={o.state} label={null} />{:else}<span class="dot-gap"></span>{/if}
                  <span class="txt">{o.label}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        <span class="grow"></span>
        {@render sideFoot()}
      </aside>
      <div class="pane">
        {#if !auto && !error}
          <div class="loading"><Loader name="orbit" size={32} label="Loading automation" /></div>
        {:else if $automationTab === 'rules'}
    <div class="body" role="tabpanel" aria-label="Rules" data-testid="automation-rules">
          <StartRules />
        </div>
        {:else if $automationTab === 'runs'}
    <div class="body" role="tabpanel" aria-label="Runs" data-testid="automation-runs">
          {#if runsError && !runs}
            <LoadError title="Couldn't load the runs" error={runsError} onretry={() => retry(loadRuns)} {retrying} testid="automation-runs-error" />
          {:else if runs && runs.length === 0}
            <p class="none">{runsOutcome ? 'No run ended this way.' : "Nothing has run on the fleet's behalf yet."}</p>
          {:else if runs}
            <ul class="table">
              {#each runs as run (run.id)}
                <li data-testid="automation-run" data-outcome={run.outcome}>
                  <StatusDot state={RUN_DOT[run.outcome] ?? 'idle'} label={outcomeLabel(run)} />
                  <span class="when tnum">{timeAgo(run.started_at, nowSec * 1000)}</span>
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
                  {:else}<span></span>{/if}
                </li>
              {/each}
            </ul>
            {#if runsTotal > runs.length}<p class="none">Showing the latest {runs.length} of {runsTotal}.</p>{/if}
          {/if}
        </div>
        {:else if $automationTab === 'agents'}
    <div class="body" role="tabpanel" aria-label="Agents" data-testid="automation-agents">
          <p class="hint">The agents fleet runs itself. Background agents live here only.</p>
          <ul class="table agents">
            {#each agents as a (a.id)}
              <li data-testid="automation-agent-{a.id}">
                <StatusDot state={a.state === 'off' ? 'idle' : 'done'} label={null} />
                <span class="main">
                  <strong>{a.name}</strong>
                  <span class="summary">{a.does}</span>
                </span>
                <span class="meta state">{a.state}</span>
              </li>
            {/each}
          </ul>
        </div>
        {:else if error}
          <LoadError title="Couldn't load automation" {error} onretry={() => retry(load)} {retrying} testid="automation-load-error" />
        {/if}
      </div>
    </div>
  {/if}
</section>

<style>
  .automation { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .fill { flex: 1; min-height: 0; }
  .split { flex: 1; min-height: 0; display: grid; grid-template-columns: var(--list-w) minmax(0, 1fr); }
  .side { display: flex; flex-direction: column; min-height: 0; overflow: auto; border-right: 1px solid var(--border); background: var(--bg-pane); }
  .pane { min-height: 0; display: flex; flex-direction: column; }
  .side-head { display: flex; flex-direction: column; gap: var(--space-2); padding: 14px var(--space-3) var(--space-2); }
  .title-row { display: flex; align-items: center; justify-content: space-between; }
  .seg { display: flex; gap: 2px; padding: 2px; background: var(--bg-raise); border: 1px solid var(--control-border, var(--border)); border-radius: var(--radius-sm); }
  .seg button { flex: 1 1 0; height: 22px; display: inline-flex; align-items: center; justify-content: center; gap: 4px; border: 0; border-radius: var(--radius-xs); background: transparent; color: var(--fg-2); font: inherit; font-size: var(--text-xs); cursor: pointer; white-space: nowrap; }
  .seg button:hover { color: var(--fg); }
  .seg button[aria-selected='true'] { background: var(--bg-hover); color: var(--fg); font-weight: 500; }
  .side-foot { position: sticky; bottom: 0; margin-top: auto; display: flex; align-items: center; gap: var(--space-2); padding: 10px var(--space-3); border-top: 1px solid var(--border); background: var(--bg-pane); }
  h1 { margin: 0; font-size: var(--text-lg); line-height: 22px; font-weight: 600; }
  .grow { flex: 1; }
  .today { font-size: var(--text-xs); color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .today.over { color: var(--status-failed); }
  .banner { margin: 0; padding: var(--space-2) var(--space-4); font-size: var(--text-sm); background: color-mix(in srgb, var(--status-waiting) 14%, transparent); }
  .err { margin: 0; padding: var(--space-2) var(--space-4); font-size: var(--text-sm); color: var(--status-failed); }
  .loading { display: flex; justify-content: center; padding: var(--space-6); }
  .body { flex: 1; min-height: 0; overflow: auto; padding: var(--space-4) var(--space-6); display: flex; flex-direction: column; gap: var(--space-3); }
  .hint, .none { margin: 0; font-size: var(--text-sm); color: var(--fg-muted); }
  .table { list-style: none; margin: 0; padding: 0; max-width: 960px; border: 1px solid var(--border); border-radius: var(--radius-md); overflow: hidden; }
  .table li { display: grid; grid-template-columns: 16px 80px minmax(0, 1fr) 64px 52px 64px; gap: 4px 10px; align-items: center; padding: 9px 12px; font-size: var(--text-sm); }
  .table li + li { border-top: 1px solid var(--border); }
  .table.agents li { grid-template-columns: 16px minmax(0, 1fr) auto; }
  .main { min-width: 0; display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; }
  .summary { flex-basis: 100%; font-size: var(--text-xs); color: var(--fg-muted); overflow-wrap: anywhere; }
  .outcome.failed { color: var(--status-failed); }
  .outcome { font-size: var(--text-xs); color: var(--fg-2); }
  .outcome.needs_person { color: var(--status-waiting); }
  .meta, .when, .num { font-size: var(--text-xs); color: var(--fg-muted); }
  .tnum, .num { font-variant-numeric: tabular-nums; }
  .state { flex: none; }
  .outcomes { list-style: none; margin: 0; padding: 0; }
  .outcomes .of-row { width: calc(100% - 12px); border: 0; background: none; color: var(--fg); font: inherit; text-align: left; cursor: pointer; align-items: center; }
  .outcomes .of-row:hover { background: var(--bg-hover); }
  .outcomes .of-row[aria-selected='true'] { background: var(--accent-soft); }
  .outcomes .of-row :global(.of-dot) { margin-top: 0; }
  .dot-gap { width: 8px; flex: none; }
  .txt { flex: 1 1 auto; font-size: var(--text-sm); line-height: 18px; }
</style>
