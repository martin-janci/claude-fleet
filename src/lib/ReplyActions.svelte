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
  import { rewindConversation, sendPrompt } from './sessions';
  import { insertIntoComposer, type ConvTurn } from './conversation';
  import { replyActionsFor, quoteText } from './reply_actions';

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

  let confirming = $state<'rewind' | 'retry' | null>(null);
  let busy = $state(false);

  // The one sentence that must not be softened: this is where fleet diverges
  // from Claude Code's own /rewind, which restores files from its checkpoints.
  const REWIND_COPY =
    'The conversation is rewound to before this turn. Your files are left as they are.';

  async function doRewind(retry: boolean) {
    busy = true;
    const r = await rewindConversation(sessionId, 'rewind', view.rewindAnchor);
    busy = false;
    confirming = null;
    if (!r.ok) return;
    if (retry && prompt) {
      // sendPrompt(hostAlias, tmuxName, prompt) — see `src/lib/sessions.ts:598`.
      // `send_prompt` has no session-id form.
      await sendPrompt(hostAlias, tmuxName, prompt);
    } else if (prompt) {
      insertIntoComposer(sessionId, prompt);
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
    title="Quote"
    onclick={() => insertIntoComposer(sessionId, quoteText(text))}>❝</button
  >
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-retry"
      aria-label="Retry this turn"
      title="Retry — rewind and send the same prompt again"
      disabled={busy}
      onclick={() => (confirming = 'retry')}>↻</button
    >
  {/if}
  {#if view.canFork}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-fork"
      aria-label="Fork a new session from this reply"
      title="Fork here"
      onclick={() => onFork(view.forkAnchor)}>⑂</button
    >
  {/if}
  {#if view.canRewind}
    <button
      type="button"
      class="btn btn--icon btn--quiet"
      data-testid="reply-rewind"
      aria-label="Rewind this session to before this turn"
      title="Rewind here"
      disabled={busy}
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
