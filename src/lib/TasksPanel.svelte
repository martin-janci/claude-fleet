<script lang="ts">
  import { tasks, cancelTask, isTerminal, promptFirstLine, taskElapsed, type TaskRow } from './tasks';
  import { sessions, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection, connectionBanner } from './hub_connection';
  import { sessionIdBlocked } from './share';

  // cancel_task routes to the hub, so it stays enabled on a hub client — but
  // only once the live connection to it is up; while it is not, sending it
  // would just wait on a socket that is not there.
  const cancelBlocked = $derived(hubActionBlocked('cancel_task', $hubStatus, $hubConnection));
  /**
   * The access half (multi-user M1, F2a). A `TaskRow` names its two parties by
   * session id only, so each id has to be resolved to a row before the access
   * half can be asked — the same shape `WorkReview` and `TidyReview` have.
   *
   * A task is a relationship between two sessions, so its parties are narrowed
   * per target the way a fan-out list is: `cancel_task` is `drive` in
   * `share.ts::SESSION_TIER` (the inverse of `dispatch_task`, which has no UI
   * surface of its own), and a cancel writes the task's end onto BOTH parties'
   * timelines, so a party shared with this client at `watch` — or not shared at
   * all — refuses the cancel.
   *
   * ── Not knowing is not permission (F2d) ─────────────────────────────────
   *
   * F2a resolved both ids out of `$sessions` by hand and answered `null` when
   * NEITHER resolved, reading an unresolvable party as "the session was reaped,
   * so there is nothing to refuse". This panel is the worst place in the app for
   * that reading: `$tasks` is the FLEET-WIDE `list_tasks`, so on a paired
   * desktop it carries tasks between sessions this client holds no rows for at
   * all — the hatch opened in exactly the case it should have closed.
   *
   * So each NAMED party goes through `share.ts::sessionIdBlocked`, which fails
   * closed with `UNKNOWN_SESSION_REASON` on a fleet this client does not own and
   * answers `null` on a standalone desktop (`access.ts::sessionAccess` rule 1).
   * A `null` id is skipped, and that is not the hatch coming back: `null` means
   * the task names no such party (the column is `ON DELETE SET NULL`), which is
   * a different fact from "there is a party and this app cannot see whose it
   * is". A task with no party at all is still asked about — `sessionIdBlocked`
   * answers for a `null` id too — so the degenerate row refuses rather than
   * sails through.
   */
  function cancelAccessBlocked(t: TaskRow): string | null {
    const named = [t.requester_session_id, t.worker_session_id].filter((id) => id !== null);
    if (named.length === 0) return $sessionIdBlocked(null, 'cancel_task');
    for (const id of named) {
      const why = $sessionIdBlocked(id, 'cancel_task');
      if (why) return why;
    }
    return null;
  }
  /** One task's own gate: the hub's refusal first, then this client's access. */
  function taskCancelBlocked(t: TaskRow): string | null {
    return cancelBlocked ?? cancelAccessBlocked(t);
  }

  // Same honest-empty-state fix as Sidebar/HostsList: `list_tasks` fails
  // with `E_HUB_CONTRACT` under a skewed hub, discarded like every other
  // "heals itself" failure (`void loadTasks()` in App.svelte), so `$tasks`
  // never arrives and this would otherwise read as "no tasks" rather than
  // "this couldn't load".
  const hubSkewEmptyMessage = $derived(
    $hubConnection.state === 'hub_too_old' || $hubConnection.state === 'hub_too_new'
      ? connectionBanner($hubConnection, $hubStatus.url)
      : null,
  );

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
  /** Open a task party's session — a deliberate "open session" click. */
  function openSession(id: number | null): void {
    const row = sessionRow(id);
    if (row) selectSessionExplicitly(row);
  }

  const STATE_COLOR: Record<TaskRow['state'], string> = {
    queued: 'var(--status-idle)',
    running: 'var(--status-working)',
    done: 'var(--status-done)',
    failed: 'var(--status-failed)',
    cancelled: 'var(--status-waiting)',
  };

  let pendingCancel: TaskRow | null = $state(null);
  let busy = $state(false);
  async function doCancel() {
    // Re-asked on confirm: the dialog can be open when a revoke arrives.
    if (!pendingCancel || taskCancelBlocked(pendingCancel) !== null) return;
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
      {hubSkewEmptyMessage ??
        (sessionId === null ? 'No tasks dispatched yet.' : 'No tasks involve this session.')}
    </p>
  {:else}
    <ul class="list">
      {#each rows as t (t.id)}
        <li class="row" data-testid="task-row" data-state={t.state}>
          <div class="head">
            <span
              class="pill"
              data-testid="task-state"
              style="color: {STATE_COLOR[t.state]}; border-color: color-mix(in srgb, {STATE_COLOR[t.state]} 40%, transparent); background: color-mix(in srgb, {STATE_COLOR[t.state]} 9%, transparent);"
            >{t.state}</span>
            <span class="id">#{t.id}</span>
            <span class="parties" data-testid="task-parties">
              {#if sessionRow(t.requester_session_id)}
                <button class="link" onclick={() => openSession(t.requester_session_id)}>{sessionLabel(t.requester_session_id)}</button>
              {:else}
                <span>{sessionLabel(t.requester_session_id)}</span>
              {/if}
              <span class="arrow">→</span>
              {#if sessionRow(t.worker_session_id)}
                <button class="link" onclick={() => openSession(t.worker_session_id)}>{sessionLabel(t.worker_session_id)}</button>
              {:else}
                <span>{sessionLabel(t.worker_session_id)}</span>
              {/if}
            </span>
            <span class="elapsed" data-testid="task-elapsed" title={t.finished_at ? 'duration' : 'elapsed'}>{taskElapsed(t, nowSec)}</span>
            {#if !isTerminal(t.state)}
              {@const why = taskCancelBlocked(t)}
              <button
                class="cancel"
                data-testid="task-cancel"
                disabled={why !== null}
                onclick={() => {
                  // The gate again, not only on `disabled` (F2d): a synthetic or
                  // scripted click reaches the handler past the attribute, and
                  // `doCancel` re-asking would leave a confirm dialog open whose
                  // button can never work. Refuse to open it instead.
                  if (why === null) pendingCancel = t;
                }}
                title={why ?? 'Cancel this task (the worker keeps running)'}
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
    font-size: 11px;
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
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .id { color: var(--fg-muted); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 11px; }
  .parties { display: inline-flex; gap: 0.3rem; align-items: baseline; min-width: 0; flex: 1; overflow: hidden; white-space: nowrap; }
  .arrow { color: var(--fg-muted); }
  .link {
    background: transparent; border: none; padding: 0; cursor: pointer;
    color: var(--accent); font-size: 0.8rem; text-decoration: underline; text-underline-offset: 2px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .elapsed { color: var(--fg-muted); font-size: 11px; }
  .cancel {
    font-size: 11px; padding: 0.15rem 0.5rem; border-radius: 4px; cursor: pointer;
    border: 1px solid var(--danger); color: var(--danger); background: transparent;
  }
  .cancel:hover { background: color-mix(in srgb, var(--danger) 10%, transparent); }
  .prompt { margin: 0; color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .result, .error {
    margin: 0; font-size: 11px; line-height: 1.35; white-space: pre-wrap; overflow-wrap: anywhere;
    max-height: 5.5rem; overflow: auto; padding: 0.3rem 0.45rem; border-radius: 4px;
  }
  .result { background: rgba(60, 180, 90, 0.1); color: var(--fg); }
  .error { background: color-mix(in srgb, var(--danger) 10%, transparent); color: var(--danger); }
</style>
