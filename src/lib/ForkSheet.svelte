<script module lang="ts">
  /** Shown for an older hub's E_UNSUPPORTED on a new-worktree fork. */
  export const NEW_WORKTREE_OLD_HUB =
    "This hub can't fork into a new worktree yet — update fleet-hub, or pick Same worktree.";
</script>

<script lang="ts">
  // Fork's confirmation sheet. Opened from ReplyActions' fork button via
  // ConversationPanel's `openForkSheet`; this dialog IS the confirmation —
  // there is no second "are you sure?" on top of it (spec §5.2).
  //
  // New worktree is the default: two live Claude sessions editing one
  // checkout is the standard way to lose work. The backend creates the
  // worktree first (a new branch at this session's HEAD), writes the
  // truncated transcript under its path, then starts the session there —
  // so uncommitted changes stay with this session, and the note says so.
  // A hub older than this build answers E_UNSUPPORTED for it; that is shown
  // as "update the hub", with Same worktree still one click away.
  import Loader from './Loader.svelte';
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { rewindConversation } from './sessions';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';
  import { finalizeBranchSlug, validateBranchName } from './branch-slug';

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

  let choice = $state<'new' | 'same'>('new');
  // Prefill only — a live-changing suggestion while the sheet is open would
  // stomp on whatever the user typed, so this is deliberately a one-time
  // snapshot, not a binding to the prop.
  let worktreeName = $state(untrack(() => suggestedName));
  let busy = $state(false);
  let error = $state<string | null>(null);

  const slug = $derived(finalizeBranchSlug(worktreeName));
  const nameProblem = $derived.by(() => {
    if (choice !== 'new') return null;
    if (slug === 'main' || slug === 'master') return 'The worktree name cannot be main or master.';
    return validateBranchName(slug);
  });

  /**
   * The hub link can drop while the sheet is open — and so can a grant
   * (multi-user M1, F2a). `rewind_conversation` is `own` in
   * `share.ts::SESSION_TIER`: a fork leaves a permanent verbatim copy of the
   * owner's transcript behind, and creates a worktree and a branch on the
   * owner's host, so it is barred for a `drive` grantee too.
   *
   * The resolution is `share.ts::sessionIdBlocked`'s (F2e), because this sheet
   * is handed a `sessionId` and nothing else: it resolves the row the same way
   * a hand-rolled `$sessions.find` did, but a MISS fails closed with
   * `UNKNOWN_SESSION_REASON` instead of handing `$sessionBlocked` an `undefined`
   * — which answers `null` = allowed, collapsing this gate to the hub half
   * alone on exactly the two rows a fork must not be offered for (somebody
   * else's, or gone). A standalone desktop still answers `null`, where the
   * master owns every row.
   *
   * The button that opens it (`ReplyActions`' Fork, through
   * `ConversationPanel`) composes the same pair, but this sheet IS the
   * confirmation — there is no second one — so a reason arriving while it is
   * open has to reach Fork itself, and `fork()` re-reads it rather than
   * trusting the disabled attribute.
   */
  const blocked = $derived(
    hubActionBlocked('rewind_conversation', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(sessionId, 'rewind_conversation'),
  );
  const canSubmit = $derived(!busy && !blocked && nameProblem === null);

  async function fork() {
    if (!canSubmit) return;
    busy = true;
    error = null;
    const r = await rewindConversation(sessionId, 'fork', anchor, choice === 'new' ? slug : null);
    busy = false;
    if (!r.ok) {
      error = r.error.code === 'E_UNSUPPORTED' && choice === 'new' ? NEW_WORKTREE_OLD_HUB : r.error.message;
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
    <div class="new-worktree-fields">
      <label for="fork-worktree-name">worktree and branch name</label>
      <input
        id="fork-worktree-name"
        data-testid="fork-worktree-name"
        value={worktreeName}
        oninput={(e) => (worktreeName = (e.target as HTMLInputElement).value)}
        disabled={busy || choice !== 'new'}
      />
      {#if nameProblem}
        <p class="problem" data-testid="fork-name-problem">{nameProblem}</p>
      {/if}
      <p class="note" data-testid="fork-new-note">
        Branches off this session's last commit. Uncommitted changes stay here — commit them first to take
        them along.
      </p>
    </div>

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

  {#if error}<p class="err" data-testid="fork-error">{error}</p>{:else if blocked}<p class="err">{blocked}</p>{/if}

  <div class="actions">
    <button type="button" onclick={onclose} disabled={busy}>Cancel</button>
    <button
      type="button"
      class="primary"
      data-testid="fork-confirm"
      disabled={!canSubmit}
      onclick={() => void fork()}>{#if busy}<Loader name="comet" size={12} class="btn-loader" />{/if}{busy ? 'Forking…' : 'Fork'}</button
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
  .note {
    margin: 0.2rem 0 0;
    font-size: 0.8em;
    color: var(--fg-muted, #999);
  }
  .problem {
    margin: 0.2rem 0 0;
    font-size: 0.8em;
    color: var(--danger, #e5534b);
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
