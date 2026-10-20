<script lang="ts">
  // The Inbox's failed routine runs (Orbit Fleet redesign 8.6): each routine
  // whose newest run failed, under the sessions that need you, with Fix
  // (its named fix when the hub names one, G3.8: open the host, the
  // accounts, its session or its editor; else open its session, or its
  // definition when it never started one), Retry
  // (run it now) and Pause. Either Retry or Pause takes it out; its count is
  // part of the Needs you badge (`failingCount`). A routine's name opens it
  // in Automation's Routines tab (8.4).
  import { shortAge } from '../session_status';
  import { push, pushError } from '../toasts';
  import {
    failing,
    fixLabel,
    fixRoutine,
    openRoutines,
    pauseRoutine,
    retryRoutine,
    runWords,
    type FailingRoutine,
  } from '../routines';

  // One id per call in flight (review w11-13): a single id let the first
  // call's return re-enable a second item's buttons mid-call.
  let busy = $state<ReadonlySet<number>>(new Set());
  const setBusy = (id: number, on: boolean) => {
    const next = new Set(busy);
    if (on) next.add(id);
    else next.delete(id);
    busy = next;
  };

  // `trackFailingRoutines` (main.ts) keeps the list fresh; opening the Inbox
  // reads it once more (Sidebar.svelte), so it is current as it shows.

  async function retry(f: FailingRoutine) {
    setBusy(f.routine.id, true);
    const r = await retryRoutine(f);
    setBusy(f.routine.id, false);
    if (!r.ok) pushError(r.error, `Retry ${f.routine.name} failed`);
    else push({ kind: 'success', message: `${f.routine.name} is running again` });
  }

  async function pause(f: FailingRoutine) {
    setBusy(f.routine.id, true);
    const r = await pauseRoutine(f);
    setBusy(f.routine.id, false);
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
              <button type="button" class="btn btn--chip" data-testid="routine-failure-fix" title={f.fix ? `${f.fix.code}${f.fix.host ? ` on ${f.fix.host}` : ''}` : undefined} onclick={() => fixRoutine(f)}>{fixLabel(f.fix)}</button>
              <button type="button" class="btn btn--chip" data-testid="routine-failure-retry" disabled={busy.has(f.routine.id)} onclick={() => retry(f)}
                >Retry</button
              >
              {#if f.routine.enabled}
                <button type="button" class="btn btn--chip" data-testid="routine-failure-pause" disabled={busy.has(f.routine.id)} onclick={() => pause(f)}
                  >Pause</button
                >
              {/if}
            </span>
          {:else}
            <!-- Review w11-6: not counted in the badge; its owner acts. -->
            <span class="line" data-testid="routine-failure-owner">Only its owner can change it</span>
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
