<script lang="ts">
  import Modal from './Modal.svelte';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { moveBlockedReason, moveTargetsFor } from './moveEligibility';
  import { describeMoveError } from './moveErrors';
  import type { ResolveAction } from './moveSession';
  import { stepLabel } from './moveProgress';
  import { dismissMove, displaySteps, moves, resolveMoveRun, retryMove, startMove, transferSheetFor } from './moves';
  import { selectSessionExplicitly } from './selection';
  import { sessions } from './sessions';

  // One sheet for the whole app. Which view shows is a function of the run:
  // none → setup, running → progress, done → result, failed/partial → failure.
  const id = $derived($transferSheetFor);
  const run = $derived(id === null ? undefined : $moves.get(id));
  const session = $derived(id === null ? undefined : $sessions.find((s) => s.id === id));
  const targets = $derived(session ? moveTargetsFor(session, $hosts) : []);
  const blocked = $derived(moveBlockedReason($hubStatus, $hubConnection));
  /** A refusal from the last Finish/Undo attempt, carried on the run itself
   *  (`moves.ts`'s `settleResolve`) rather than local component state, so it
   *  is naturally per-session: switching the sheet to another run's `id`
   *  reads that run's own `resolveError`, never a leftover from this one. */
  const resolveError = $derived(run?.resolveError ?? null);

  let target = $state('');
  let keepSource = $state(false);
  let showDetails = $state(false);
  // Which destructive action is one click from happening. Cleared whenever the
  // sheet's session changes, like `showDetails`.
  let confirming = $state<'clean' | 'finish' | 'undo' | null>(null);

  // A fresh setup each time the sheet opens on a session.
  $effect(() => {
    if (id !== null && !run) {
      if (!targets.some((h) => h.alias === target)) target = targets[0]?.alias ?? '';
    }
  });
  $effect(() => {
    void id;
    keepSource = false;
    showDetails = false;
    confirming = null;
  });
  // Nothing to show: the row is gone and no run remembers it.
  $effect(() => {
    if (id !== null && !run && !session) transferSheetFor.set(null);
  });

  // What the steps list shows: a settled run completes its own picture at
  // render time rather than by rewriting what its events reported.
  const shown = $derived(run ? displaySteps(run) : []);
  /** The last step that is not pending: how far the move actually got. */
  const reached = $derived(shown.findLast((s) => s.state !== 'pending')?.step ?? null);
  const failure = $derived(
    run && (run.status === 'failed' || run.status === 'partial')
      ? describeMoveError(run.error, run.status, run.toHost, reached)
      : null,
  );
  const carried = $derived(run?.report?.carried ?? null);
  /** Narrows `failure.action` for the template, which otherwise cannot keep
   *  `.paths` in scope across the `{#if}` that tests `.kind`. */
  const cleanAction = $derived(failure?.action?.kind === 'clean' ? failure.action : null);

  /** Whether this partial names a target `resolve_move` could act on. The two
   *  earliest partial steps record no target id at all (the row does not
   *  exist yet), so Finish and Undo have nothing to kill and the backend
   *  refuses both — offering a red "Kill the new session on {host}" that can
   *  only answer "no target session to resolve" is worse than offering
   *  nothing. Read the same way `moves.ts`'s own `targetIdOf` reads it. */
  const resolvableTargetId = $derived.by(() => {
    if (run?.report) return run.report.target_session_id;
    const details = run?.error?.details;
    if (typeof details === 'object' && details !== null) {
      const v = (details as Record<string, unknown>).target_session_id;
      if (typeof v === 'number') return v;
    }
    return null;
  });

  /** Everything the move left behind, from all three lists, keyed by both —
   *  the same path can be left behind by two of them. */
  const leftBehind = $derived(
    carried
      ? [
          ...carried.ignored_left_behind.map((f) => ({ ...f, key: `ignored:${f.path}` })),
          ...carried.session_state.left_behind.map((f) => ({ ...f, key: `session:${f.path}` })),
          ...carried.memory.left_behind.map((f) => ({ ...f, key: `memory:${f.path}` })),
        ]
      : [],
  );

  /** The session a finished move produced, when this window can find it. */
  const newSession = $derived.by(() => {
    if (!run) return undefined;
    if (run.report) return run.report.target;
    const details = run.error?.details;
    const tid =
      typeof details === 'object' && details !== null
        ? (details as Record<string, unknown>).target_session_id
        : undefined;
    if (typeof tid === 'number') return $sessions.find((s) => s.id === tid);
    return $sessions.find((s) => s.parent_session_id === run.sessionId && s.host_alias === run.toHost);
  });

  /** The CURRENT row for a finished move's target, read fresh from `$sessions`
   *  by `report.target_session_id` — never the snapshot embedded in the
   *  report, which can be stale by the time "Move back" is clicked. The
   *  source row is gone by now, so this is what a move back actually moves;
   *  no row here means nothing left to move back.
   *
   *  `source_killed` is the gate: a `keep_source` move left the ORIGIN
   *  running this very conversation, in the same worktree, so a move "back"
   *  there would aim the transfer at that live session's own worktree. The
   *  engine refuses it (`E_INVALID_STATE`); the sheet does not offer it. */
  const moveBackTarget = $derived.by(() => {
    if (!run?.report?.source_killed) return undefined;
    const targetId = run.report.target_session_id;
    return $sessions.find((s) => s.id === targetId);
  });

  const n = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;
  const REASON = {
    denylisted: 'never carried (secrets, caches, build output)',
    over_cap: 'over the size cap',
    unsupported_name: 'a file name that cannot be carried safely',
  } as const;

  function close() {
    transferSheetFor.set(null);
  }
  function transfer() {
    if (!session || !target || blocked !== null) return;
    startMove(session, target, { keepSource });
  }
  function done() {
    if (run) dismissMove(run.sessionId);
    close();
  }
  function openTarget() {
    if (newSession) selectSessionExplicitly(newSession);
    done();
  }
  function moveBack(): void {
    if (!moveBackTarget || !run) return;
    startMove(moveBackTarget, run.fromHost, { keepSource: false });
  }
  function retry(cleanTarget: boolean): void {
    if (id === null) return;
    confirming = null;
    retryMove(id, { cleanTarget });
  }
  function resolve(action: ResolveAction): void {
    if (id === null) return;
    confirming = null;
    // `resolveMoveRun` clears any stale `resolveError` on the run itself
    // before making a fresh attempt.
    resolveMoveRun(id, action);
  }

  const title = $derived(
    !run
      ? `Transfer ${session?.tmux_name ?? ''}`
      : run.status === 'running'
        ? `Moving to ${run.toHost}`
        : run.status === 'done'
          ? `Moved to ${run.toHost}`
          : 'The move did not finish',
  );
</script>

{#snippet steps()}
  {#if run}
    <ol class="steps" data-testid="transfer-steps">
      {#each shown as s (s.step)}
        <li data-state={s.state}>
          <span class="mark" aria-hidden="true"></span>
          <span class="label">{stepLabel(s.step, run.toHost)}</span>
          {#if s.detail}<span class="detail">{s.detail}</span>{/if}
        </li>
      {/each}
    </ol>
  {/if}
{/snippet}

{#if id !== null && (run || session)}
  <Modal {title} onclose={close} width="480px" testid="move-dialog">
    {#if !run && session}
      <div class="field"><span class="key">From</span> {session.host_alias}</div>
      {#if targets.length === 0}
        <p class="note" data-testid="move-no-targets">No other reachable, provisioned host.</p>
      {:else}
        <label class="field">
          <span class="key">To</span>
          <select bind:value={target} data-testid="move-target">
            {#each targets as h (h.alias)}
              <option value={h.alias}>{h.alias}</option>
            {/each}
          </select>
        </label>
        <label class="field">
          <input type="checkbox" bind:checked={keepSource} data-testid="move-keep-source" />
          Keep this session running
        </label>
      {/if}
      <p class="note">
        Uncommitted and unpushed work, small ignored files, subagents and project memory travel
        too. Nothing is pushed or committed.
      </p>
      <div class="buttons">
        <button onclick={close}>Cancel</button>
        <button
          onclick={transfer}
          disabled={!target || blocked !== null}
          title={blocked ?? ''}
          data-testid="confirm-move"
        >
          Transfer
        </button>
      </div>
    {:else if run && run.status === 'running'}
      {#if run.error}
        <p class="note">Lost contact with the hub — the move may still be running there.</p>
        <!-- The same code covers a refused connection and a 404, which are
             not "may still be running" at all: show what was actually said. -->
        <p class="note" data-testid="transfer-lost-contact-detail">{run.error.message}</p>
      {:else if run.origin === 'observed'}
        <p class="note">Started elsewhere — this window is following along.</p>
      {/if}
      {@render steps()}
      <div class="buttons">
        {#if run.origin === 'observed'}
          <button onclick={done} data-testid="transfer-stop-following">Stop following</button>
        {/if}
        <button onclick={close} data-testid="transfer-close">Close</button>
      </div>
    {:else if run && run.status === 'done'}
      <div data-testid="transfer-result">
        {#if carried}
          <ul class="summary">
            <li>
              {#if carried.commits === 0 && carried.dirty_entries.length === 0}
                Nothing to carry — the branch was pushed and clean
              {:else}
                {n(carried.commits, 'commit')} · {n(carried.dirty_entries.length, 'uncommitted entry', 'uncommitted entries')}
              {/if}
            </li>
            <li>
              {n(carried.ignored_carried.length, 'ignored file')}
              {#if carried.ignored_left_behind.length > 0}
                <span class="muted">· {carried.ignored_left_behind.length} left behind</span>
              {/if}
            </li>
            <li>
              {n(carried.session_state.carried.length, 'session file')}
              {#if carried.session_state.kept_target.length > 0}
                <span class="muted">· {carried.session_state.kept_target.length} kept on target</span>
              {/if}
              {#if carried.session_state.left_behind.length > 0}
                <span class="muted">· {carried.session_state.left_behind.length} left behind</span>
              {/if}
            </li>
            <li>
              {n(carried.memory.carried.length, 'memory note')}
              {#if carried.memory.kept_target.length + carried.memory.identical > 0}
                <span class="muted">· {carried.memory.kept_target.length + carried.memory.identical} already there</span>
              {/if}
              {#if carried.memory.index_lines_added > 0}
                <span class="muted">· {n(carried.memory.index_lines_added, 'index line')}</span>
              {/if}
            </li>
          </ul>
          {#if run.report && !run.report.source_killed}
            <p class="note">The source session keeps running on {run.fromHost}.</p>
          {/if}
          {#if run.report && run.report.warnings.length > 0}
            <div class="warnings">
              <p class="warn-head">{n(run.report.warnings.length, 'warning')}</p>
              <!-- By index: two warnings can read exactly the same. -->
              {#each run.report.warnings as w, i (i)}
                <p class="warn" data-testid="transfer-warning">{w}</p>
              {/each}
            </div>
          {/if}
          {#if showDetails}
            <div class="details">
              {#each leftBehind as f (f.key)}
                <p><code>{f.path}</code> — {REASON[f.reason] ?? f.reason}</p>
              {/each}
              {#each carried.session_state.kept_target as p (p)}
                <p><code>{p}</code> — kept the target's copy</p>
              {/each}
              {#each carried.memory.kept_target as p (p)}
                <p><code>{p}</code> — kept the target's note</p>
              {/each}
              {#if carried.memory.identical > 0}
                <p>{n(carried.memory.identical, 'note')} identical on both hosts</p>
              {/if}
            </div>
          {/if}
        {:else if run.error}
          <p class="note">
            The connection to the hub was lost during the move, so this window has no report for it.
          </p>
        {:else}
          <p class="note">Started elsewhere — this window has no report for it.</p>
        {/if}
      </div>
      <div class="buttons">
        {#if carried}
          <button onclick={() => (showDetails = !showDetails)} data-testid="transfer-details">
            {showDetails ? 'Hide details' : 'Details'}
          </button>
        {/if}
        {#if newSession}
          <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
        {/if}
        {#if run.fromHost && moveBackTarget}
          <button onclick={moveBack} data-testid="transfer-move-back">Move back to {run.fromHost}</button>
        {/if}
        <button onclick={done} data-testid="transfer-done">Done</button>
      </div>
    {:else if run && failure}
      <div data-testid="transfer-failure">
        <p class="what">{failure.what}</p>
        <p class="note">{failure.standing}</p>
        {@render steps()}
        {#if run.error}
          <details>
            <summary>Raw details</summary>
            <pre>{run.error.code}
{run.error.message}{#if typeof run.error.details === 'object' && run.error.details !== null && typeof (run.error.details as Record<string, unknown>).stderr === 'string'}

{(run.error.details as Record<string, unknown>).stderr}{/if}</pre>
          </details>
        {/if}
        {#if resolveError}
          <p class="note" data-testid="transfer-resolve-error">{resolveError.message}</p>
        {/if}
        {#if confirming === 'clean' && cleanAction}
          <ul class="clean-paths">
            {#each cleanAction.paths as p (p)}
              <li><code>{p}</code></li>
            {/each}
            {#if cleanAction.more > 0}
              <li class="muted">+{cleanAction.more} more</li>
            {/if}
          </ul>
        {/if}
      </div>
      <div class="buttons">
        {#if run.status === 'partial'}
          {#if newSession}
            <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
          {/if}
          {#if resolvableTargetId !== null}
            {#if confirming === 'finish'}
              <button
                class="danger"
                onclick={() => resolve('finish')}
                disabled={run.resolving}
                data-testid="transfer-finish-confirm"
              >
                Kill {run.sessionName} on {run.fromHost}
              </button>
            {:else}
              <button onclick={() => (confirming = 'finish')} data-testid="transfer-finish">Finish the move</button>
            {/if}
            {#if confirming === 'undo'}
              <button
                class="danger"
                onclick={() => resolve('undo')}
                disabled={run.resolving}
                data-testid="transfer-undo-confirm"
              >
                Kill the new session on {run.toHost}
              </button>
            {:else}
              <button onclick={() => (confirming = 'undo')} data-testid="transfer-undo">Undo</button>
            {/if}
          {/if}
        {:else if cleanAction}
          {#if confirming === 'clean'}
            <button class="danger" onclick={() => retry(true)} data-testid="transfer-clean-confirm">
              Replace {cleanAction.paths.length + cleanAction.more} file(s) on {run.toHost} and retry
            </button>
          {:else}
            <button onclick={() => (confirming = 'clean')} data-testid="transfer-clean">
              Clean up {run.toHost} and retry
            </button>
          {/if}
        {:else if failure.action?.kind === 'retry'}
          <button onclick={() => retry(false)} data-testid="transfer-retry">Retry</button>
        {/if}
        <button onclick={done} data-testid="transfer-done">Done</button>
      </div>
    {/if}
  </Modal>
{/if}

<style>
  .field { display: flex; align-items: center; gap: 0.5rem; margin: 0.35rem 0; font-size: 0.85rem; }
  .key { width: 3rem; color: var(--fg-muted); }
  .field select { flex: 1; }
  .note, .muted { color: var(--fg-muted); font-size: 0.8rem; }
  .what { font-size: 0.9rem; margin: 0 0 0.35rem; }
  .steps, .summary { list-style: none; margin: 0.5rem 0; padding: 0; font-size: 0.85rem; }
  .steps li { display: flex; align-items: baseline; gap: 0.5rem; padding: 0.15rem 0; }
  .steps li[data-state='pending'] { color: var(--fg-muted); }
  .steps li[data-state='failed'] .label { color: #e64a4a; }
  .steps li[data-state='warned'] .label { color: #d29b4a; }
  .mark { width: 1rem; text-align: center; }
  .steps li[data-state='pending'] .mark::before { content: '○'; }
  .steps li[data-state='started'] .mark::before { content: '◌'; }
  .steps li[data-state='done'] .mark::before { content: '✓'; }
  .steps li[data-state='warned'] .mark::before { content: '!'; }
  .steps li[data-state='failed'] .mark::before { content: '✕'; }
  .detail { margin-left: auto; color: var(--fg-muted); font-size: 0.75rem; }
  .summary li { padding: 0.15rem 0; }
  .warnings { border-top: 1px solid var(--border); margin-top: 0.5rem; padding-top: 0.4rem; }
  .warn-head { color: #d29b4a; font-size: 0.85rem; margin: 0; }
  .warn, .details p { font-size: 0.8rem; color: var(--fg-muted); margin: 0.15rem 0; }
  .details { border-top: 1px solid var(--border); margin-top: 0.5rem; padding-top: 0.4rem; }
  pre { white-space: pre-wrap; font-size: 0.75rem; }
  .clean-paths { list-style: none; margin: 0.4rem 0; padding: 0; font-size: 0.8rem; max-height: 8rem; overflow-y: auto; }
  .clean-paths li { padding: 0.1rem 0; }
  .clean-paths code { font-size: 0.75rem; }
  .buttons { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.75rem; }
  .buttons .danger { color: #e64a4a; }
</style>
