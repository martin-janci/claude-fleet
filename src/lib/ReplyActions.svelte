<script lang="ts">
  // The action row under one reply. Always visible, never hover-revealed —
  // CopyButton's own comment says why: a control you must hover to find is
  // not a control a keyboard or touch user has.
  //
  // Retry is deliberately not a third backend mode. It is a rewind followed
  // by a send, so it inherits every refusal the rewind has (including the
  // mid-turn one) instead of keeping a second copy of them in step.
  import CopyButton from './CopyButton.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { rewindConversation } from './sessions';
  import {
    CONV_MAX_TURNS,
    carriedCount,
    insertIntoComposer,
    promptCount,
    sessionConversation,
    splitMarker,
    type ConvTurn,
  } from './conversation';
  import { outbox } from './outbox';
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
  // Without the hub's untrusted-client marker: the transcript keeps the line
  // `apply_marker` prepended, and a re-send through the hub would mark it
  // a second time (and put it in the composer, on a rewind).
  const prompt = $derived.by(() => {
    const p = turns[index]?.prompt;
    return p == null ? null : splitMarker(p).text;
  });

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
    // Through the outbox, the session's one sender: a bubble with receipts,
    // and a failed send offers Retry / Edit / Discard there. `send_prompt`
    // has no session-id form, so the target carries host + tmux name.
    const seen = await retrySeen(r.value.claude_session_id ?? undefined, prompt);
    outbox.enqueue(
      { id: sessionId, host_alias: hostAlias, tmux_name: tmuxName },
      { kind: 'prompt', text: prompt, prefix: null, seen },
    );
  }

  // `seen` for the re-sent prompt: how many turns of the REWOUND conversation
  // already carry it. That conversation holds only the turns BEFORE this one,
  // so the stale pre-rewind transcript's copy must not count. With the whole
  // conversation on screen those turns are `turns.slice(0, index)`. A
  // truncated window hides older ones, and the rewound conversation's first
  // read can bring them into view — a hidden earlier "continue" would then
  // settle the bubble before Claude got the prompt. So the count comes from
  // the rewound conversation itself, read at the widest window any panel read
  // can return (a panel's window is a suffix of it), which is the composer's
  // rule (`carriedCount`). If that read fails, only the quiet-turn fallback
  // may settle the bubble.
  async function retrySeen(cid: string | undefined, body: string): Promise<number> {
    if (!truncated) return promptCount(turns.slice(0, index), body);
    const r = await sessionConversation(sessionId, CONV_MAX_TURNS, cid);
    return r.ok ? carriedCount(r.value, body) : Number.MAX_SAFE_INTEGER;
  }
</script>

<div class="reply-actions" role="group" aria-label="Actions for this reply">
  <CopyButton {text} label="Copy reply" />
  <button
    type="button"
    class="btn btn--icon btn--quiet"
    data-testid="reply-quote"
    aria-label="Quote this reply in the composer"
    title="Quote"
    onclick={() => insertIntoComposer(sessionId, quoteText(text))}>❝</button
  >
  {#if view.canRetry}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-retry"
      aria-label="Retry this turn"
      title={retryBlocked ?? 'Retry — rewind and send the same prompt again'}
      disabled={busy || !!retryBlocked}
      onclick={() => (confirming = 'retry')}>↻</button
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
      disabled>↻</button
    >
  {/if}
  {#if view.canFork}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-fork"
      aria-label="Fork a new session from this reply"
      title={rewindBlocked ?? 'Fork here'}
      disabled={!!rewindBlocked}
      onclick={() => onFork(view.forkAnchor)}>⑂</button
    >
  {/if}
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-rewind"
      aria-label="Rewind this session to before this turn"
      title={rewindBlocked ?? 'Rewind here'}
      disabled={busy || !!rewindBlocked}
      onclick={() => (confirming = 'rewind')}>⏪</button
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
  }
</style>
