<!--
  Control's Orbit view (automation runs board C, in the Thread room's right
  panel): the orbit map of the last day's runs, compact. A comet opens the
  run's steps here in the panel; Open session puts its session in focus;
  ⤢ opens the full orbit in Automation's Runs.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import OrbitMap from './automation/OrbitMap.svelte';
  import RunTimeline from './automation/RunTimeline.svelte';
  import { RUN_CHANGED_EVENT } from './events';
  import { hosts as hostRows } from './hosts';
  import { sessions as sessionRows, type SessionRow } from './sessions';
  import { listRoutines, type RoutineRow } from './routines';
  import { listRuns, type RunRow } from './runs';
  import { windowHidden } from './window_hidden';
  import { readErrorText } from './work_view';
  import type { IpcError } from './result';

  let { onfocus }: { onfocus?: (s: SessionRow) => void } = $props();

  /** The orbit's window: what ran in the last day. */
  const WINDOW_SECS = 24 * 3600;
  /** A missed run event, or a routine's next fire moving on, shows within this. */
  const REFRESH_MS = 60_000;

  let runs = $state<RunRow[] | null>(null);
  let routines = $state<RoutineRow[]>([]);
  let loadError = $state<IpcError | null>(null);
  let nowSec = $state(Math.floor(Date.now() / 1000));
  let openRunId = $state<string | null>(null);

  async function load() {
    const r = await listRuns({ limit: 200, since: Math.floor(Date.now() / 1000) - WINDOW_SECS });
    if (r.ok) {
      runs = r.value.runs;
      loadError = null;
    } else loadError = r.error;
  }

  const loadRoutines = () => listRoutines().then((r) => (routines = r.ok && Array.isArray(r.value) ? r.value : routines));

  onMount(() => {
    void load();
    void loadRoutines();
    const tick = setInterval(() => {
      if (!windowHidden()) nowSec = Math.floor(Date.now() / 1000);
    }, 1000);
    const refresh = setInterval(() => {
      if (windowHidden()) return;
      void load();
      void loadRoutines();
    }, REFRESH_MS);
    // A run recorded a step: re-read, at most once a second and a half.
    let pending: ReturnType<typeof setTimeout> | null = null;
    let un: (() => void) | null = null;
    let gone = false;
    void listen(RUN_CHANGED_EVENT, () => {
      if (pending) return;
      pending = setTimeout(() => {
        pending = null;
        void load();
      }, 1500);
    })
      .then((u) => (gone ? u() : (un = u)))
      .catch(() => {});
    return () => {
      gone = true;
      un?.();
      if (pending) clearTimeout(pending);
      clearInterval(tick);
      clearInterval(refresh);
    };
  });

  const openedRun = $derived(openRunId ? (runs?.find((r) => r.id === openRunId) ?? null) : null);

  function openSession(run: RunRow) {
    const id = run.session_ids[0];
    const s = id === undefined ? undefined : $sessionRows.find((x) => x.id === id);
    if (s) onfocus?.(s);
  }
</script>

<div class="orbit-view" data-testid="control-orbit">
  {#if openedRun}
    <RunTimeline run={openedRun} {nowSec} onclose={() => (openRunId = null)} onopensession={openSession} />
  {:else if loadError && !runs}
    <p class="muted" role="alert" data-testid="control-orbit-error">
      {readErrorText(loadError)} <button type="button" class="btn" onclick={() => void load()}>Retry</button>
    </p>
  {:else if runs}
    <OrbitMap
      compact
      {runs}
      hostAliases={$hostRows.map((h) => h.alias)}
      sessions={$sessionRows}
      {routines}
      now={nowSec}
      onpick={(r) => (openRunId = r.id)}
    />
  {/if}
</div>

<style>
  .orbit-view {
    padding: var(--space-3);
    overflow: auto;
    min-height: 0;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .btn {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 8px;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
