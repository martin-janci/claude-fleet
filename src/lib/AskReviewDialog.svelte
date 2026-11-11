<script lang="ts">
  // "Ask for review…" (docs/review-notes.md): the session's owner asks a
  // colleague on this hub to review its changes before anything is pushed.
  // Says plainly what the colleague gets: the session shared with them —
  // its changes, the notes and the conversation, before and after the ask —
  // and the ask in their Inbox. A member can send notes to the agent; a
  // viewer reads and drafts.
  //
  // On DialogSheet: while the ask is in flight the sheet cannot be closed
  // (the hub may already be sharing), a failure is its banner, and a typed
  // person or note asks before it is discarded.
  import DialogSheet from './DialogSheet.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { askForReview, levelWords } from './review_requests';
  import type { IpcError } from './result';
  import { push } from './toasts';
  import type { SessionRow } from './sessions';

  let { session, onclose }: { session: SessionRow; onclose: () => void } = $props();

  let person = $state('');
  let note = $state('');
  let busy = $state(false);
  let error = $state<IpcError | null>(null);

  const blocked = $derived(hubActionBlocked('review_request', $hubStatus, $hubConnection));
  const why = $derived(blocked ?? (person.trim() === '' ? 'Name who should review.' : null));
  const dirty = $derived(person.trim() !== '' || note.trim() !== '');

  async function submit() {
    if (busy || why !== null) return;
    busy = true;
    error = null;
    const r = await askForReview(session.id, person, note);
    busy = false;
    if (!r.ok) {
      error = r.error;
      return;
    }
    push({ kind: 'info', message: `Asked ${r.value.to_name} to review; they ${levelWords(r.value.level)}.` });
    onclose();
  }
</script>

<DialogSheet
  title="Ask for review"
  lead="Share this session with a colleague and put the ask in their Inbox."
  verb="Ask"
  busyVerb="Asking…"
  {busy}
  {error}
  {dirty}
  canConfirm={why === null}
  confirmTitle={why}
  onconfirm={() => void submit()}
  {onclose}
  width="440px"
  testid="ask-review-dialog"
  confirmTestid="ask-review-send"
  errorTestid="ask-review-error"
>
  <label class="field">
    <span class="field-label">Who</span>
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
    <span class="field-label">Note <span class="opt">optional</span></span>
    <textarea rows="3" bind:value={note} placeholder="What to look at" data-testid="ask-review-note"></textarea>
  </label>
  <p class="field-note" data-testid="ask-review-explain">
    They get this session shared with them: its changes, the review notes and the whole conversation, from before this
    ask too. "{session.friendly_name || session.tmux_name}" shows in their Inbox until they mark it done. A member can
    send notes to the agent; a viewer reads and drafts. You can take the share back in Share….
  </p>
</DialogSheet>

<style>
  .opt {
    color: var(--fg-muted);
    font-style: italic;
    margin-left: 0.3em;
  }
</style>
