<script lang="ts">
  // The start's progress under the Work button (task → session P-5): the
  // steps the start writes to its session's timeline, live, with Cancel
  // start (P-6) until the agent has its brief. History first, then the live
  // events, merged by id, so a step written before the strip mounted is
  // not missed.
  import { get } from 'svelte/store';
  import { sessionHistory, type SessionEvent } from './timeline';
  import { onTimelineEvent } from './live_events';
  import { sessions } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { sessionIdBlocked } from './share';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { abandonStart } from './trackers';
  import { readErrorText } from './work_view';
  import { progressText, startSteps } from './start_progress';

  let { sessionId, onclose }: { sessionId: number; onclose: () => void } = $props();

  let events = $state<SessionEvent[]>([]);
  let busy = $state(false);
  let error = $state<string | null>(null);
  const seen = new Set<number>();
  function add(list: SessionEvent[]) {
    const fresh = list.filter((e) => !seen.has(e.id));
    for (const e of fresh) seen.add(e.id);
    if (fresh.length) events = [...events, ...fresh];
  }

  $effect(() => {
    const id = sessionId;
    const off = onTimelineEvent(id, (e) => add([e]));
    void sessionHistory(id, 50).then((r) => {
      if (r.ok && Array.isArray(r.value)) add(r.value);
    });
    return off;
  });

  const progress = $derived(startSteps(events));
  const alive = $derived($sessions.some((r) => r.id === sessionId));
  const cancelBlocked = $derived(
    hubActionBlocked('abandon_start', $hubStatus, $hubConnection) ?? $sessionIdBlocked(sessionId, 'abandon_start'),
  );

  // Done: let it be read, then step aside.
  let doneTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (progress?.done && !progress.waiting && !error) doneTimer = setTimeout(onclose, 6000);
    return () => clearTimeout(doneTimer);
  });

  function open() {
    const row = get(sessions).find((r) => r.id === sessionId);
    if (row) selectSessionExplicitly(row);
  }

  async function cancel() {
    if (busy || cancelBlocked) return;
    busy = true;
    error = null;
    const r = await abandonStart(sessionId);
    busy = false;
    if (r.ok) onclose();
    else error = readErrorText(r.error);
  }
</script>

{#if progress && (alive || error)}
  <span class="sp" data-testid="start-progress">
    <span class="steps">
      {#each progress.steps as s (s.id)}
        <span class="step step--{s.state}" data-testid="start-step" data-step={s.id} data-state={s.state}
          >{s.state === 'done' ? '✓' : s.state === 'active' ? '…' : '○'} {s.label}</span
        >
      {/each}
    </span>
    <span class="caption" class:caption--warn={progress.waiting || progress.failed} role="status" aria-live="polite"
      >{progressText(progress)}</span
    >
    <span class="actions">
      {#if progress.waiting}
        <button class="btn btn--quiet" type="button" data-testid="start-progress-open" onclick={open}>Open</button>
      {/if}
      {#if progress.cancellable}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="start-progress-cancel"
          disabled={busy || cancelBlocked !== null}
          title={cancelBlocked ?? 'End this session and remove the checkout and branch it made'}
          onclick={() => void cancel()}>{busy ? '…' : 'Cancel start'}</button
        >
      {/if}
      <button class="btn btn--quiet" type="button" aria-label="Hide start progress" onclick={onclose}>×</button>
    </span>
    {#if error}<span class="err" role="alert" data-testid="start-progress-error">{error}</span>{/if}
  </span>
{/if}

<style>
  .sp {
    display: grid;
    justify-items: end;
    gap: 2px;
    max-width: 320px;
    font-size: 11px;
  }
  .steps {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: flex-end;
  }
  .step--done {
    color: var(--fg);
  }
  .step--active {
    color: var(--accent);
  }
  .step--pending {
    color: var(--fg-muted);
  }
  .caption {
    color: var(--fg-muted);
    text-align: right;
  }
  .caption--warn {
    color: var(--usage-warn, #b26a00);
  }
  .actions {
    display: inline-flex;
    gap: 4px;
  }
  .err {
    color: var(--usage-crit, #c62828);
    text-align: right;
  }
</style>
