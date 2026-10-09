<script lang="ts">
  // The Inbox's failed routine runs (Orbit Fleet redesign 8.6): each routine
  // whose newest run failed, under the sessions that need you, with Fix
  // (open its session, or its definition when it never started one), Retry
  // (run it now) and Pause. Either Retry or Pause takes it out; its count is
  // part of the Needs you badge (`failingCount`). A routine's name opens it
  // in Automation's Routines tab (8.4).
  import { onMount } from 'svelte';
  import { shortAge } from '../session_status';
  import { push, pushError } from '../toasts';
  import {
    failing,
    fixRoutine,
    loadFailing,
    openRoutines,
    pauseRoutine,
    retryRoutine,
    runWords,
    type FailingRoutine,
  } from '../routines';

  let busy = $state<number | null>(null);

  // `trackFailingRoutines` (main.ts) keeps the list fresh; opening the Inbox
  // reads it once more so it is current as it shows.
  onMount(() => void loadFailing());

  async function retry(f: FailingRoutine) {
    busy = f.routine.id;
    const r = await retryRoutine(f);
    busy = null;
    if (!r.ok) pushError(r.error, `Retry ${f.routine.name} failed`);
    else push({ kind: 'success', message: `${f.routine.name} is running again` });
  }

  async function pause(f: FailingRoutine) {
    busy = f.routine.id;
    const r = await pauseRoutine(f);
    busy = null;
    if (!r.ok) pushError(r.error, `Pause ${f.routine.name} failed`);
    else push({ kind: 'success', message: `${f.routine.name} paused` });
  }
</script>

{#if $failing.length > 0}
  <div class="failures" data-testid="routine-failures" role="group" aria-label="Failed routine runs">
    {#each $failing as f (f.routine.id)}
      <div class="item" data-testid="routine-failure">
        <span class="dot" aria-label="Failed"></span>
        <div class="text">
          <button type="button" class="name" data-testid="routine-failure-open" onclick={() => openRoutines({ select: f.routine.id, tab: 'runs' })}
            >{f.routine.name} <span class="chip">routine</span></button
          >
          <span class="line">{runWords(f.run)} · {shortAge(f.run.finished_at ?? f.run.started_at)}</span>
          {#if f.routine.paused_reason}<span class="line">{f.routine.paused_reason}</span>{/if}
          {#if f.may_change}
            <span class="actions">
              <button type="button" class="btn btn--chip" data-testid="routine-failure-fix" onclick={() => fixRoutine(f)}>Fix</button>
              <button type="button" class="btn btn--chip" data-testid="routine-failure-retry" disabled={busy === f.routine.id} onclick={() => retry(f)}
                >Retry</button
              >
              {#if f.routine.enabled}
                <button type="button" class="btn btn--chip" data-testid="routine-failure-pause" disabled={busy === f.routine.id} onclick={() => pause(f)}
                  >Pause</button
                >
              {/if}
            </span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .failures { display: flex; flex-direction: column; gap: 2px; padding: 4px 8px; }
  .item { display: flex; gap: 8px; align-items: flex-start; padding: 6px 4px; border-radius: var(--radius-md); }
  .item:hover { background: var(--bg-hover); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--status-failed); margin-top: 5px; flex: none; }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .name { border: 0; background: none; padding: 0; color: var(--fg); font: inherit; font-weight: 500; text-align: left; cursor: pointer; }
  .chip { font-size: var(--text-2xs); color: var(--fg-muted); border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 6px; font-weight: 400; }
  .line { color: var(--fg-muted); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .actions { display: flex; gap: 6px; margin-top: 4px; }
</style>
