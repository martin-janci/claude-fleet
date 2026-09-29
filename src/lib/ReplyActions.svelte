<script lang="ts">
  // The action row under one reply: a footer after the turn's last text,
  // in the flow (it used to float over the text's top-right corner and cover
  // words). Always visible, never hover-revealed — CopyButton's own comment
  // says why: a control you must hover to find is not a control a keyboard
  // or touch user has. It rests dimmed and comes up on hover / focus, and
  // the clipboard pair (Copy, Quote) is kept apart from the three that
  // change the session (Retry, Fork, Rewind).
  //
  // Retry is deliberately not a third backend mode. It is a rewind followed
  // by a send, so it inherits every refusal the rewind has (including the
  // mid-turn one) instead of keeping a second copy of them in step.
  import CopyButton from './CopyButton.svelte';
  import Icon from './Icon.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { rewindConversation, sendPrompt } from './sessions';
  import { insertIntoComposer, type ConvTurn } from './conversation';
  import { replyActionsFor, quoteText, waitForReplQuiet } from './reply_actions';
  import { pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';

  let {
    turns,
    index,
    truncated,
    text,
    sessionId,
    hostAlias,
    tmuxName,
    supported = true,
    onFork,
  }: {
    turns: ConvTurn[];
    index: number;
    truncated: boolean;
    text: string;
    sessionId: number;
    /** `sendPrompt` addresses a session by host + tmux name, not by id. */
    hostAlias: string;
    tmuxName: string;
    supported?: boolean;
    /** Opens the worktree sheet (Task 7); it calls the backend itself. */
    onFork: (anchor: string | null) => void;
  } = $props();

  const view = $derived(replyActionsFor(turns, index, truncated, supported));
  const prompt = $derived(turns[index]?.prompt ?? null);

  // Both route to the hub: with its link down they would fail with a raw
  // error, so they say why instead (as the composer does for send_prompt).
  const rewindBlocked = $derived(
    hubActionBlocked('rewind_conversation', $hubStatus, $hubConnection),
  );
  const retryBlocked = $derived(
    rewindBlocked ?? hubActionBlocked('send_prompt', $hubStatus, $hubConnection),
  );

  let confirming = $state<'rewind' | 'retry' | null>(null);
  let busy = $state(false);

  // The one sentence that must not be softened: this is where fleet diverges
  // from Claude Code's own /rewind, which restores files from its checkpoints.
  const REWIND_COPY =
    'The conversation is rewound to before this turn. Your files are left as they are.';

  // Every refusal the engine can make — the mid-turn `E_INVALID`, a
  // compacted-away anchor's `E_NOTFOUND`, `E_NO_TRANSCRIPT`, `E_BG_SESSION`,
  // `E_SELF_TARGET` — used to close the dialog and do nothing at all.
  // `invokeCmd` has no global error surfacing, so it has to happen here, the
  // way `SessionDetails.svelte:197` does it for `restartSession` (and
  // `ForkSheet.svelte:62` for the fork).
  async function doRewind(retry: boolean) {
    const label = retry ? 'Retry failed' : 'Rewind failed';
    busy = true;
    const r = await rewindConversation(sessionId, 'rewind', view.rewindAnchor);
    if (!r.ok) {
      busy = false;
      confirming = null;
      pushError(r.error, label);
      return;
    }
    if (!retry) {
      busy = false;
      confirming = null;
      if (prompt) insertIntoComposer(sessionId, prompt);
      return;
    }
    // The pane was respawned milliseconds ago and `send_prompt` is a blind
    // tmux paste + Enter: a send now lands in a pty not yet in raw mode, so
    // the prompt is dropped, mangled, or the Enter answers a trust screen.
    // `spawn_review` waits for the same readiness before its seed send. The
    // wait is BOUNDED, and on timeout nothing is sent: the prompt goes to the
    // composer with an error, so it is never silently lost.
    const ready = await waitForReplQuiet(sessionId);
    busy = false;
    confirming = null;
    if (!prompt) return;
    if (!ready) {
      insertIntoComposer(sessionId, prompt);
      pushError(
        {
          code: 'E_TIMEOUT',
          message:
            'the session was rewound, but its REPL did not come back in time, so the prompt was not resent — it is in the composer',
        },
        'Retry',
      );
      return;
    }
    // sendPrompt(hostAlias, tmuxName, prompt) — see `src/lib/sessions.ts:598`.
    // `send_prompt` has no session-id form.
    const sent = await sendPrompt(hostAlias, tmuxName, prompt);
    if (!sent.ok) {
      insertIntoComposer(sessionId, prompt);
      pushError(sent.error, 'Retry: the session was rewound but the prompt was not resent');
    }
  }
</script>

<div class="reply-actions" role="group" aria-label="Actions for this reply">
  <CopyButton {text} label="Copy reply" />
  <button
    type="button"
    class="btn btn--icon btn--quiet"
    data-testid="reply-quote"
    aria-label="Quote this reply in the composer"
    title="Quote in the composer"
    onclick={() => insertIntoComposer(sessionId, quoteText(text))}><Icon name="quote" size={14} /></button
  >
  {#if view.canRetry || view.retryUnavailable || view.canFork || view.canRewind}
    <span class="sep" aria-hidden="true"></span>
  {/if}
  {#if view.canRetry}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-retry"
      aria-label="Retry this turn"
      title={retryBlocked ?? 'Retry — rewind and send the same prompt again'}
      disabled={busy || !!retryBlocked}
      onclick={() => (confirming = 'retry')}><Icon name="retry" size={14} /></button
    >
  {:else if view.retryUnavailable}
    <!-- Shown, not dropped: the prompt on screen is not what a re-send would
         send (cut to fit, or it held an image), and the tooltip says so. -->
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-retry"
      aria-label="Retry this turn (unavailable)"
      title={view.retryUnavailable}
      disabled><Icon name="retry" size={14} /></button
    >
  {/if}
  {#if view.canFork}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-fork"
      aria-label="Fork a new session from this reply"
      title={rewindBlocked ?? 'Fork here — a new session from this reply'}
      disabled={!!rewindBlocked}
      onclick={() => onFork(view.forkAnchor)}><Icon name="fork" size={14} /></button
    >
  {/if}
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet rewind"
      data-testid="reply-rewind"
      aria-label="Rewind this session to before this turn"
      title={rewindBlocked ?? 'Rewind here — back to before this turn'}
      disabled={busy || !!rewindBlocked}
      onclick={() => (confirming = 'rewind')}><Icon name="rewind" size={14} /></button
    >
  {/if}
</div>

{#if confirming}
  <ConfirmDialog
    title={confirming === 'retry' ? 'Retry this turn?' : 'Rewind to before this turn?'}
    message={confirming === 'retry'
      ? `${REWIND_COPY} The same prompt is then sent again.`
      : `${REWIND_COPY} The prompt returns to the composer.`}
    confirmLabel={confirming === 'retry' ? 'Retry' : 'Rewind'}
    danger
    {busy}
    onconfirm={() => void doRewind(confirming === 'retry')}
    oncancel={() => (confirming = null)}
    confirmTestId="confirm-ok"
  />
{/if}

<style>
  .reply-actions {
    display: flex;
    gap: 2px;
    align-items: center;
    opacity: 0.6;
    transition: opacity 120ms ease;
  }
  /* The whole turn wakes the row, not just the row itself: the pointer is
     on the reply, and a keyboard user tabbing in lands on a button. */
  :global(.turn:hover) .reply-actions,
  .reply-actions:focus-within {
    opacity: 1;
  }
  /* Touch has no hover: never leave the row dimmed there. */
  @media (pointer: coarse) {
    .reply-actions {
      opacity: 1;
    }
  }
  .reply-actions :global(.btn--icon) {
    width: 26px;
    height: 26px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 5px;
    color: var(--fg-muted);
  }
  .reply-actions :global(.btn--icon:hover:not(:disabled)) {
    color: var(--fg);
  }
  .reply-actions :global(.btn--icon:disabled) {
    opacity: 0.4;
  }
  .reply-actions .rewind:hover:not(:disabled) {
    color: var(--usage-warn, #d29922);
  }
  .sep {
    width: 1px;
    height: 14px;
    margin: 0 6px;
    background: var(--border, currentColor);
    opacity: 0.6;
  }
</style>
