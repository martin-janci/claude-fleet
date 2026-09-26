<script lang="ts">
  // Fork's confirmation sheet (Task 7). Opened from ReplyActions' fork
  // button via ConversationPanel's `openForkSheet`; this dialog IS the
  // confirmation — there is no second "are you sure?" on top of it.
  //
  // New worktree is the default: two live Claude sessions editing one
  // checkout is the standard way to lose work, so making the SAFE choice
  // the one you have to opt out of (not opt into) is deliberate.
  //
  // The backend does not implement a new-worktree fork yet:
  // `rewind_conversation` refuses `mode: fork` + `new_worktree: Some(_)`
  // with `E_UNSUPPORTED` (see crates/fleet-core/src/service/rewind.rs — a
  // not-yet-created worktree's physical path is only known after
  // `new_session` creates it, which is too late for the transcript rewrite
  // that has to happen first). So the option stays visible and stays the
  // default (it is still the right answer once it lands), but Fork itself
  // is disabled while it is selected — never silently downgraded to a
  // same-worktree fork, and never sent to the backend to fail server-side.
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { rewindConversation } from './sessions';

  let {
    sessionId,
    anchor,
    suggestedName,
    onclose,
  }: {
    sessionId: number;
    /** Fork's truncation anchor — `null` keeps the whole transcript. */
    anchor: string | null;
    /** Prefill for the new worktree's name (derived from the session). */
    suggestedName: string;
    onclose: () => void;
  } = $props();

  const NEW_WORKTREE_UNAVAILABLE =
    "Forking into a new worktree isn't available yet — the new session would start with no history. Pick Same worktree below to fork now.";

  let choice = $state<'new' | 'same'>('new');
  // Prefill only — a live-changing suggestion while the sheet is open would
  // stomp on whatever the user typed, so this is deliberately a one-time
  // snapshot, not a binding to the prop.
  let worktreeName = $state(untrack(() => suggestedName));
  let busy = $state(false);
  let error = $state<string | null>(null);

  const canSubmit = $derived(choice === 'same' && !busy);

  async function fork() {
    if (!canSubmit) return;
    busy = true;
    error = null;
    const r = await rewindConversation(sessionId, 'fork', anchor, null);
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onclose();
  }
</script>

<Modal title="Fork this reply" onclose={busy ? undefined : onclose} width="440px" testid="fork-sheet">
  <p class="hint">
    Starts a new session on this reply's history. This session is left running, unchanged.
  </p>

  <fieldset class="choices">
    <legend class="sr-only">Worktree for the new session</legend>

    <label class="choice">
      <input
        type="radio"
        name="fork-worktree"
        data-testid="fork-new-worktree"
        checked={choice === 'new'}
        disabled={busy}
        onchange={() => (choice = 'new')}
      />
      <span class="choice-label">New worktree <span class="recommended">(recommended)</span></span>
    </label>
    {#if choice === 'new'}
      <div class="new-worktree-fields">
        <label for="fork-worktree-name">worktree name</label>
        <input
          id="fork-worktree-name"
          data-testid="fork-worktree-name"
          value={worktreeName}
          oninput={(e) => (worktreeName = (e.target as HTMLInputElement).value)}
          disabled
        />
        <p class="unavailable" data-testid="fork-new-unavailable">{NEW_WORKTREE_UNAVAILABLE}</p>
      </div>
    {/if}

    <label class="choice">
      <input
        type="radio"
        name="fork-worktree"
        data-testid="fork-same-worktree"
        checked={choice === 'same'}
        disabled={busy}
        onchange={() => (choice = 'same')}
      />
      <span class="choice-label" data-testid="fork-same-warning"
        >Same worktree — ⚠ both sessions edit the same files</span
      >
    </label>
  </fieldset>

  {#if error}<p class="err">{error}</p>{/if}

  <div class="actions">
    <button type="button" onclick={onclose} disabled={busy}>Cancel</button>
    <button
      type="button"
      class="primary"
      data-testid="fork-confirm"
      disabled={!canSubmit}
      title={choice === 'new' ? NEW_WORKTREE_UNAVAILABLE : ''}
      onclick={() => void fork()}>{busy ? 'Forking…' : 'Fork'}</button
    >
  </div>
</Modal>

<style>
  .hint {
    margin: 0 0 0.6rem;
    font-size: 0.85em;
    color: var(--fg-muted, #999);
  }
  .choices {
    border: none;
    padding: 0;
    margin: 0 0 0.6rem;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .choice {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    padding: 0.3rem 0;
    cursor: pointer;
  }
  .choice-label {
    line-height: 1.3;
  }
  .recommended {
    color: var(--fg-muted, #999);
    font-size: 0.85em;
  }
  .new-worktree-fields {
    margin: 0 0 0.3rem 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .new-worktree-fields label {
    font-size: 0.8em;
    color: var(--fg-muted, #999);
  }
  .unavailable {
    margin: 0.2rem 0 0;
    font-size: 0.8em;
    color: var(--warn, #f59e0b);
  }
  .err {
    color: var(--danger, #e5534b);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.8rem;
  }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button.primary {
    border-color: var(--accent);
  }
  .actions button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
