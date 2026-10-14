<!-- A routine's Debug tab (migration 186, `service::routines::debug`): why it
     does or does not fire, check by check; the events an event routine would
     read, each with what it does to it; the scheduler's decision trace,
     dropped and filtered events included; and Dry run, what a fire now would
     start and the exact prompt it would type. Nothing here starts a run. -->
<script lang="ts">
  import Button from '../kit/Button.svelte';
  import StatusDot from '../kit/StatusDot.svelte';
  import { errorText } from '../error_copy';
  import type { IpcError } from '../result';
  import {
    checkLabel,
    debugDot,
    decisionLabel,
    dryRunRoutine,
    explainRoutine,
    type RoutineDryRun,
    type RoutineExplain,
  } from '../routines';

  let { routineId, clock }: { routineId: number; clock: (s: number) => string } = $props();

  let explain = $state<RoutineExplain | null>(null);
  let dry = $state<RoutineDryRun | null>(null);
  let error = $state<IpcError | null>(null);
  let loading = $state(false);
  let seq = 0;

  async function load() {
    const mine = ++seq;
    loading = true;
    const r = await explainRoutine(routineId, 50);
    if (mine !== seq) return;
    loading = false;
    if (r.ok) {
      explain = r.value;
      error = null;
    } else {
      error = r.error;
    }
  }

  async function dryRun(eventId?: number) {
    const r = await dryRunRoutine(routineId, eventId);
    if (r.ok) {
      dry = r.value;
      error = null;
    } else {
      error = r.error;
    }
  }

  $effect(() => {
    void routineId;
    explain = null;
    dry = null;
    void load();
  });
</script>

<div class="debug" data-testid="routine-debug">
  <div class="bar">
    {#if explain}
      <StatusDot state={debugDot(explain.state)} label={null} />
      <strong data-testid="routine-debug-summary">{explain.summary}</strong>
    {:else if loading}
      <span class="meta">Asking the scheduler…</span>
    {/if}
    <span class="grow"></span>
    <Button variant="quiet" testid="routine-debug-refresh" disabled={loading} onclick={() => void load()}>Refresh</Button>
    <Button testid="routine-debug-dry-run" onclick={() => void dryRun()}>Dry run</Button>
  </div>
  {#if error}<p class="err" role="alert" data-testid="routine-debug-error">{errorText(error)}</p>{/if}

  {#if dry}
    <section data-testid="routine-dry-run">
      <h4 class="sub">Dry run <span class="meta">nothing was started</span></h4>
      <p class="line" class:bad={!dry.starts}>
        {#if dry.starts}
          A fire now starts a session on {dry.session.host_alias}{dry.session.profile ? ` as profile ${dry.session.profile}` : ''}.{#if dry.note}{' '}{dry.note}.{/if}
        {:else}
          A fire now is skipped: {dry.reason}.
        {/if}
      </p>
      {#if dry.event}
        <p class="line meta" data-testid="routine-dry-run-event">
          Tried on {dry.event.kind} #{dry.event.event_id} (session {dry.event.session_id}): {dry.event.fires ? 'it fires the routine' : `it does not fire: ${dry.event.reason}`}
        </p>
      {/if}
      {#each dry.scheduled_only.filter((c) => c.state !== 'ok') as c (c.name)}
        <p class="line meta">A scheduled fire also needs {checkLabel(c.name).toLowerCase()}: {c.detail}.</p>
      {/each}
      <p class="prompt mono" data-testid="routine-dry-run-prompt">{dry.prompt}</p>
    </section>
  {/if}

  {#if explain}
    <section>
      <h4 class="sub">Checks <span class="meta">in the order the scheduler asks</span></h4>
      <ul class="list" data-testid="routine-debug-checks">
        {#each explain.checks as c (c.name)}
          <li data-testid={`routine-check-${c.name}`}>
            <StatusDot state={debugDot(c.state)} label={c.state === 'ok' ? 'Passes' : c.state === 'warn' ? 'Passes, with a caveat' : 'Stops a fire'} />
            <span class="name">{checkLabel(c.name)}</span>
            <span class="what">{c.detail}</span>
          </li>
        {/each}
      </ul>
    </section>

    {#if explain.event}
      <section>
        <h4 class="sub">
          Recent {explain.event.kind} events
          <span class="meta">read up to #{explain.event.cursor}{explain.event.unread !== undefined ? ` · ${explain.event.unread} unread` : ''}</span>
        </h4>
        <ul class="list" data-testid="routine-debug-events">
          {#each explain.event.recent as e (e.event_id)}
            <li>
              <StatusDot state={e.fires ? 'done' : 'idle'} label={e.fires ? 'Fires it' : 'Does not fire it'} />
              <span class="name tnum">#{e.event_id} · {clock(e.at)}</span>
              <span class="what">{e.fires ? 'Fires it' : e.reason}{e.read ? ' · read' : ''}</span>
              <Button variant="quiet" size="sm" testid="routine-debug-try" onclick={() => void dryRun(e.event_id)}>Try</Button>
            </li>
          {:else}
            <li class="none meta">No {explain.event.kind} event you can see yet.</li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h4 class="sub">Decisions <span class="meta">what the scheduler did, newest first</span></h4>
      <ul class="list" data-testid="routine-debug-decisions">
        {#each explain.decisions as d (d.id)}
          <li>
            <StatusDot state={debugDot(d.verdict)} label={null} />
            <span class="name tnum">{clock(d.at)}</span>
            <span class="what"><strong>{decisionLabel(d.verdict)}</strong>{d.reason ? ` · ${d.reason}` : ''}{d.trigger_ref?.startsWith('retry:') ? ' · retry' : ''}</span>
          </li>
        {:else}
          <li class="none meta">No decisions yet.</li>
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  .debug { display: flex; flex-direction: column; gap: var(--space-4); }
  .bar { display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap; }
  .grow { flex: 1 1 auto; }
  .sub { margin: 0 0 8px; font-size: var(--text-2xs); font-weight: 500; color: var(--fg-muted); display: flex; gap: 6px; align-items: baseline; }
  .sub .meta { font-weight: 400; }
  .meta { font-size: var(--text-xs); color: var(--fg-muted); }
  .tnum { font-variant-numeric: tabular-nums; }
  .mono { font-family: var(--font-mono); font-size: var(--text-xs); }
  .err { color: var(--danger); font-size: var(--text-xs); margin: 0; }
  .line { margin: 0 0 6px; font-size: var(--text-sm); }
  .line.bad { color: var(--status-failed); }
  .list { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border); border-radius: var(--radius-md); overflow: hidden; }
  .list li { display: grid; grid-template-columns: 16px 120px minmax(0, 1fr) auto; gap: 6px 10px; align-items: center; padding: 8px 12px; font-size: var(--text-sm); }
  .list li + li { border-top: 1px solid var(--border); }
  .list li.none { display: block; }
  .name { color: var(--fg-2); }
  .what { min-width: 0; overflow-wrap: anywhere; color: var(--fg-2); }
  .prompt { white-space: pre-wrap; margin: 0; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--bg-pane); color: var(--fg-2); line-height: 18px; }
</style>
