<script lang="ts">
  // A "Paused · limit" row's two answers (redesign step 4.4), as the Main
  // board draws them: Switch account resumes the conversation under the login
  // on its host with the most headroom, after the person sees which one; Wait
  // folds the buttons into "Waiting until <reset>". Nothing moves without the
  // second press: AI proposes, a person confirms.
  import { restartSession, type SessionRow } from './sessions';
  import { push, pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionBlocked } from './share';
  import {
    loginLabel,
    resetText,
    switchTarget,
    usedText,
    waitingOut,
    waitOut,
    type HostLogin,
  } from './account_limits';
  import { nowTick } from './now_tick';

  let {
    sess,
    resetsAt,
    accountName,
  }: {
    sess: SessionRow;
    /** When the limit resets (unix seconds); `null` when the reading has none. */
    resetsAt: number | null;
    accountName: (uuid: string) => string;
  } = $props();

  // Switching is a restart, so it takes restart's access answer.
  const restartBlocked = $derived(
    hubActionBlocked('restart_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'restart_session'),
  );
  let busy = $state(false);
  let target = $state<HostLogin | null>(null);
  // A wait holds for the limit it was chosen for: while its reset is ahead,
  // or it is this limit's reset. A later limit (the row paused again after
  // the reset) asks afresh rather than "Waiting until <past time>".
  const waiting = $derived.by(() => {
    const w = $waitingOut.get(sess.id);
    if (w == null) return undefined;
    void $nowTick;
    const now = Math.floor(Date.now() / 1000);
    return w > now || (resetsAt != null && w >= resetsAt) ? w : undefined;
  });

  async function propose(e: MouseEvent) {
    e.stopPropagation();
    busy = true;
    const t = await switchTarget(sess);
    busy = false;
    if (t) target = t;
    else push({ kind: 'info', message: `No other login on ${sess.host_alias} has room left` });
  }

  async function confirm(e: MouseEvent) {
    e.stopPropagation();
    const t = target;
    if (!t || restartBlocked !== null) return;
    busy = true;
    const r = await restartSession(sess.host_alias, sess.tmux_name, t.profile ?? '');
    busy = false;
    target = null;
    if (!r.ok) pushError(r.error, 'Switching the account failed');
    else push({ kind: 'info', message: `Resumed under ${loginLabel(t, accountName)}` });
  }

  function cancel(e: MouseEvent) {
    e.stopPropagation();
    target = null;
  }

  function wait(e: MouseEvent) {
    e.stopPropagation();
    if (resetsAt != null) waitOut(sess.id, resetsAt);
  }
</script>

<div class="limit-actions" data-testid="limit-actions">
  {#if target}
    <span class="ask" data-testid="limit-switch-ask">
      Resume under {loginLabel(target, accountName)} · {usedText(target)}?
    </span>
    <button class="btn btn--chip" data-testid="limit-switch-confirm" disabled={busy} onclick={confirm}>Switch</button>
    <button class="btn btn--quiet" data-testid="limit-switch-cancel" disabled={busy} onclick={cancel}>Cancel</button>
  {:else if waiting != null}
    <span class="waiting" data-testid="limit-waiting">Waiting until {resetText(waiting)}</span>
  {:else}
    <button
      class="btn btn--chip"
      data-testid="limit-switch"
      disabled={busy || restartBlocked !== null}
      title={restartBlocked ?? 'Resume this conversation under the login on its host with the most headroom'}
      onclick={propose}
    >Switch account</button>
    {#if resetsAt != null}
      <button class="btn btn--quiet" data-testid="limit-wait" onclick={wait}>Wait until {resetText(resetsAt)}</button>
    {/if}
  {/if}
</div>

<style>
  .limit-actions {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex-wrap: wrap;
    padding-left: 0.85rem;
    margin-top: 0.2rem;
    font-size: var(--text-2xs);
  }
  .ask,
  .waiting {
    color: var(--fg-muted);
  }
</style>
