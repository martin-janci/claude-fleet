<script lang="ts">
  // "Ask for review…" (docs/review-notes.md): the session's owner asks a
  // colleague on this hub to review its changes before anything is pushed.
  // Says plainly what the colleague gets: the session shared with them —
  // its changes, the notes and the conversation, before and after the ask —
  // and the ask in their Inbox. A member can send notes to the agent; a
  // viewer reads and drafts.
  import Modal from './Modal.svelte';
  import { errorText } from './error_copy';
  import { formKeys, platformSubmitHint } from './forms/form_frame';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { askForReview, levelWords } from './review_requests';
  import { push } from './toasts';
  import type { SessionRow } from './sessions';

  let { session, onclose }: { session: SessionRow; onclose: () => void } = $props();

  let person = $state('');
  let note = $state('');
  let busy = $state(false);
  let failure = $state<string | null>(null);

  const blocked = $derived(hubActionBlocked('review_request', $hubStatus, $hubConnection));
  const canAsk = $derived(!busy && blocked === null && person.trim() !== '');

  async function submit(e?: Event) {
    e?.preventDefault();
    if (!canAsk) return;
    busy = true;
    failure = null;
    const r = await askForReview(session.id, person, note);
    busy = false;
    if (!r.ok) {
      failure = errorText(r.error);
      return;
    }
    push({ kind: 'info', message: `Asked ${r.value.to_name} to review; they ${levelWords(r.value.level)}.` });
    onclose();
  }

  const hint = platformSubmitHint();
</script>

<Modal title="Ask for review" {onclose} width="440px" testid="ask-review-dialog">
  <form class="form" use:formKeys onsubmit={submit}>
    <label class="field">
      <span>Who</span>
      <input
        type="text"
        bind:value={person}
        placeholder="Their name on this hub"
        spellcheck="false"
        autocomplete="off"
        data-autofocus=""
        data-testid="ask-review-person"
      />
    </label>
    <label class="field">
      <span>Note (optional)</span>
      <textarea rows="3" bind:value={note} placeholder="What to look at" data-testid="ask-review-note"></textarea>
    </label>
    <p class="note" data-testid="ask-review-explain">
      They get this session shared with them: its changes, the review notes and the whole conversation, from before
      this ask too. "{session.friendly_name || session.tmux_name}" shows in their Inbox until they mark it done. A member
      can send notes to the agent; a viewer reads and drafts. You can take the share back in Share….
    </p>
    {#if blocked}
      <p class="err" data-testid="ask-review-blocked">{blocked}</p>
    {/if}
    {#if failure}
      <p class="err" role="alert" data-testid="ask-review-error">{failure}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button
        type="submit"
        class="primary"
        disabled={!canAsk}
        title={blocked ?? ''}
        data-shortcut={hint.label}
        aria-keyshortcuts={hint.aria}
        data-testid="ask-review-send">{busy ? 'Asking…' : 'Ask'}</button
      >
    </div>
  </form>
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.5rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input,
  .field textarea {
    font: inherit;
    font-size: var(--text-xs);
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    resize: vertical;
  }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
