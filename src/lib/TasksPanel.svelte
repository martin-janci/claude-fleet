<script lang="ts">
  import { tasks, cancelTask, isTerminal, promptFirstLine, taskElapsed, type TaskRow } from './tasks';
  import { sessions, type SessionRow } from './sessions';
  import { selectSession } from './selection';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { pushError } from './toasts';

  // `sessionId` narrows the list to tasks where that session is the
  // requester or the worker (the SessionDetails mount); omit it for the
  // fleet-wide view opened from the sidebar header.
  let { sessionId = null }: { sessionId?: number | null } = $props();

  const rows = $derived(
    sessionId === null
      ? $tasks
      : $tasks.filter(
          (t) => t.requester_session_id === sessionId || t.worker_session_id === sessionId,
        ),
  );

  // Coarse clock for the elapsed column.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 15_000);
    return () => clearInterval(t);
  });

  const byId = $derived(new Map($sessions.map((s) => [s.id, s])));
  function sessionLabel(id: number | null): string {
    if (id === null) return '—';
    const s = byId.get(id);
    return s ? s.friendly_name || s.tmux_name : `#${id}`;
  }
  function sessionRow(id: number | null): SessionRow | null {
    return id === null ? null : (byId.get(id) ?? null);
  }

  const STATE_COLOR: Record<TaskRow['state'], string> = {
    queued: '#8a8a8a',
    running: '#4a90d2',
    done: '#3cb45a',
    failed: '#e64a4a',
    cancelled: '#d29b4a',
  };

  let pendingCancel: TaskRow | null = $state(null);
  let busy = $state(false);
  async function doCancel() {
    if (!pendingCancel) return;
    busy = true;
    const r = await cancelTask(pendingCancel.id);
    busy = false;
    pendingCancel = null;
    if (!r.ok) pushError(r.error, 'Cancel task failed');
  }
</script>

<section class="tasks" data-testid="tasks-panel" data-scope={sessionId === null ? 'fleet' : 'session'}>
  <h3>Tasks ({rows.length})</h3>
  {#if rows.length === 0}
    <p class="empty" data-testid="tasks-empty">
      {sessionId === null ? 'No tasks dispatched yet.' : 'No tasks involve this session.'}
    </p>
  {:else}
    <ul class="list">
      {#each rows as t (t.id)}
        <li class="row" data-testid="task-row" data-state={t.state}>
          <div class="head">
            <span
              class="pill"
              data-testid="task-state"
              style="color: {STATE_COLOR[t.state]}; border-color: {STATE_COLOR[t.state]}66; background: {STATE_COLOR[t.state]}18;"
            >{t.state}</span>
            <span class="id">#{t.id}</span>
            <span class="parties" data-testid="task-parties">
              {#if sessionRow(t.requester_session_id)}
                <button class="link" onclick={() => selectSession(sessionRow(t.requester_session_id))}>{sessionLabel(t.requester_session_id)}</button>
              {:else}
                <span>{sessionLabel(t.requester_session_id)}</span>
              {/if}
              <span class="arrow">→</span>
              {#if sessionRow(t.worker_session_id)}
                <button class="link" onclick={() => selectSession(sessionRow(t.worker_session_id))}>{sessionLabel(t.worker_session_id)}</button>
              {:else}
                <span>{sessionLabel(t.worker_session_id)}</span>
              {/if}
            </span>
            <span class="elapsed" data-testid="task-elapsed" title={t.finished_at ? 'duration' : 'elapsed'}>{taskElapsed(t, nowSec)}</span>
            {#if !isTerminal(t.state)}
              <button
                class="cancel"
                data-testid="task-cancel"
                onclick={() => (pendingCancel = t)}
                title="Cancel this task (the worker keeps running)"
              >Cancel</button>
            {/if}
          </div>
          <p class="prompt" data-testid="task-prompt" title={t.prompt ?? undefined}>{promptFirstLine(t.prompt)}</p>
          {#if t.result}
            <p class="result" data-testid="task-result">{t.result}</p>
          {:else if t.error}
            <p class="error" data-testid="task-error">{t.error}</p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if pendingCancel}
  <ConfirmDialog
    title="Cancel task?"
    confirmLabel="Cancel task"
    cancelLabel="Keep"
    danger
    {busy}
    onconfirm={doCancel}
    oncancel={() => (pendingCancel = null)}
    confirmTestId="confirm-cancel-task"
  >
    Task <code>#{pendingCancel.id}</code> will be marked cancelled. The worker
    session <code>{sessionLabel(pendingCancel.worker_session_id)}</code> keeps
    running — kill or re-prompt it separately if needed.
  </ConfirmDialog>
{/if}

<style>
  .tasks { display: flex; flex-direction: column; gap: 0.4rem; }
  .tasks h3 {
    margin: 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .empty { margin: 0; font-size: 0.8rem; color: var(--fg-muted); font-style: italic; }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .row {
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0.4rem 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.8rem;
  }
  .head { display: flex; align-items: center; gap: 0.45rem; flex-wrap: wrap; }
  .pill {
    padding: 0.05rem 0.4rem;
    border-radius: 999px;
    border: 1px solid;
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .id { color: var(--fg-muted); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.72rem; }
  .parties { display: inline-flex; gap: 0.3rem; align-items: baseline; min-width: 0; flex: 1; overflow: hidden; white-space: nowrap; }
  .arrow { color: var(--fg-muted); }
  .link {
    background: transparent; border: none; padding: 0; cursor: pointer;
    color: var(--accent); font-size: 0.8rem; text-decoration: underline; text-underline-offset: 2px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .elapsed { color: var(--fg-muted); font-size: 0.72rem; }
  .cancel {
    font-size: 0.72rem; padding: 0.15rem 0.5rem; border-radius: 4px; cursor: pointer;
    border: 1px solid #e64a4a; color: #e64a4a; background: transparent;
  }
  .cancel:hover { background: rgba(230, 74, 74, 0.1); }
  .prompt { margin: 0; color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .result, .error {
    margin: 0; font-size: 0.78rem; line-height: 1.35; white-space: pre-wrap; overflow-wrap: anywhere;
    max-height: 5.5rem; overflow: auto; padding: 0.3rem 0.45rem; border-radius: 4px;
  }
  .result { background: rgba(60, 180, 90, 0.1); color: var(--fg); }
  .error { background: rgba(230, 74, 74, 0.1); color: #e64a4a; }
</style>
