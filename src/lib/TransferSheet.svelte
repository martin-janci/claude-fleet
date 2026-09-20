<script lang="ts">
  import Modal from './Modal.svelte';
  import { hosts } from './hosts';
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { moveBlockedReason, moveTargetsFor } from './moveEligibility';
  import { describeMoveError } from './moveErrors';
  import { stepLabel } from './moveProgress';
  import { dismissMove, moves, startMove, transferSheetFor } from './moves';
  import { selectSession } from './selection';
  import { sessions } from './sessions';

  // One sheet for the whole app. Which view shows is a function of the run:
  // none → setup, running → progress, done → result, failed/partial → failure.
  const id = $derived($transferSheetFor);
  const run = $derived(id === null ? undefined : $moves.get(id));
  const session = $derived(id === null ? undefined : $sessions.find((s) => s.id === id));
  const targets = $derived(session ? moveTargetsFor(session, $hosts) : []);
  const blocked = $derived(moveBlockedReason($hubStatus, $hubConnection));

  let target = $state('');
  let keepSource = $state(false);
  let showDetails = $state(false);

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
  });
  // Nothing to show: the row is gone and no run remembers it.
  $effect(() => {
    if (id !== null && !run && !session) transferSheetFor.set(null);
  });

  const failure = $derived(
    run && (run.status === 'failed' || run.status === 'partial')
      ? describeMoveError(run.error, run.status, run.toHost)
      : null,
  );
  const carried = $derived(run?.report?.carried ?? null);

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
    if (newSession) selectSession(newSession);
    done();
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
      {#each run.steps as s (s.step)}
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
      {#if run.origin === 'observed'}
        <p class="note">Started elsewhere — this window is following along.</p>
      {/if}
      {@render steps()}
      <div class="buttons">
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
              {#each run.report.warnings as w (w)}
                <p class="warn" data-testid="transfer-warning">{w}</p>
              {/each}
            </div>
          {/if}
          {#if showDetails}
            <div class="details">
              {#each [...carried.ignored_left_behind, ...carried.session_state.left_behind, ...carried.memory.left_behind] as f (f.path)}
                <p><code>{f.path}</code> — {REASON[f.reason]}</p>
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
      </div>
      <div class="buttons">
        {#if run.status === 'partial' && newSession}
          <button onclick={openTarget} data-testid="transfer-open-target">Open on {run.toHost}</button>
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
  .buttons { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.75rem; }
</style>
