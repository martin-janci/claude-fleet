<script module lang="ts">
  /** Shown for an older hub's E_UNSUPPORTED on a new-worktree fork. */
  export const NEW_WORKTREE_OLD_HUB =
    "This hub can't fork into a new worktree yet — update fleet-hub, or pick Same worktree.";
</script>

<script lang="ts">
  import Icon from './kit/Icon.svelte';
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
  import { untrack } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import Loader from './Loader.svelte';
  import { rewindConversation, sessions } from './sessions';
  import { moveSession } from './moveSession';
  import { hosts } from './hosts';
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

  const source = $derived($sessions.find((s) => s.id === sessionId) ?? null);
  const sourceName = $derived(source ? (source.friendly_name ?? source.tmux_name) : 'this session');

  let choice = $state<'new' | 'same'>('new');
  // Prefill only — a live-changing suggestion while the sheet is open would
  // stomp on whatever the user typed, so this is deliberately a one-time
  // snapshot, not a binding to the prop.
  let worktreeName = $state(untrack(() => suggestedName));
  /** `null` = the source's own host. Another host forks here, then moves the
   *  fork there (step 5.10: "Fork with a host choice"). */
  let targetHost = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  /** Hosts the fork can move to: visible, reachable, not the source's. */
  const otherHosts = $derived(
    $hosts
      .filter((h) => !h.hidden && h.reachable && h.alias !== source?.host_alias)
      .map((h) => h.alias)
      .sort(),
  );
  const moving = $derived(targetHost !== null);

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
   *
   * Forking to another host also moves the fork, so `move_session`'s own
   * pair is asked too, on the source row (the fork inherits its owner).
   */
  const blocked = $derived(
    hubActionBlocked('rewind_conversation', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(sessionId, 'rewind_conversation') ??
      (moving
        ? (hubActionBlocked('move_session', $hubStatus, $hubConnection) ??
          $sessionIdBlocked(sessionId, 'move_session'))
        : null),
  );
  const canSubmit = $derived(!busy && !blocked && nameProblem === null && !notice);

  function pickHost(v: string) {
    targetHost = v === '' ? null : v;
    // Same worktree means this host's checkout: another host needs its own.
    if (targetHost !== null) choice = 'new';
  }

  async function fork() {
    if (!canSubmit) return;
    busy = true;
    error = null;
    const r = await rewindConversation(sessionId, 'fork', anchor, choice === 'new' ? slug : null);
    if (!r.ok) {
      busy = false;
      error = r.error.code === 'E_UNSUPPORTED' && choice === 'new' ? NEW_WORKTREE_OLD_HUB : r.error.message;
      return;
    }
    if (targetHost === null) {
      busy = false;
      onclose();
      return;
    }
    // The fork exists; now carry it over. A failed move leaves a working
    // fork on this host, and says so rather than hiding it behind an error.
    const m = await moveSession(r.value.id, targetHost, { when: 'idle' });
    busy = false;
    if (!m.ok) {
      error = `Forked on ${r.value.host_alias}, but the move to ${targetHost} failed: ${m.error.message}`;
      notice = 'forked';
      return;
    }
    if (m.value.kind === 'waiting') {
      notice = `Forked. It moves to ${targetHost} as soon as it is idle.`;
      return;
    }
    onclose();
  }
</script>

<DialogSheet
  title={`Fork "${sourceName}"`}
  lead="Copies the conversation up to a turn into a new session. The original never changes."
  verb={notice ? 'Done' : 'Fork'}
  busyVerb={moving ? 'Forking and moving…' : 'Forking…'}
  {busy}
  canConfirm={canSubmit || notice !== null}
  onconfirm={() => (notice ? onclose() : void fork())}
  {onclose}
  error={error ?? blocked}
  errorTestid="fork-error"
  confirmTestid="fork-confirm"
  width="460px"
  testid="fork-sheet"
>
  <div class="field">
    <span class="field-label">From turn</span>
    <span data-testid="fork-from">{anchor === null ? 'The latest turn' : 'This reply'}</span>
  </div>

  <fieldset class="field choices">
    <legend class="field-label">Into</legend>

    <label class="choice">
      <input
        type="radio"
        name="fork-worktree"
        data-testid="fork-new-worktree"
        checked={choice === 'new'}
        disabled={busy}
        onchange={() => (choice = 'new')}
      />
      <span class="choice-label">New worktree <span class="recommended">· fresh branch at HEAD</span></span>
    </label>
    <div class="new-worktree-fields">
      <label for="fork-worktree-name" class="field-label">worktree and branch name</label>
      <input
        id="fork-worktree-name"
        type="text"
        data-testid="fork-worktree-name"
        value={worktreeName}
        oninput={(e) => (worktreeName = (e.target as HTMLInputElement).value)}
        disabled={busy || choice !== 'new'}
      />
      {#if nameProblem}
        <p class="problem" data-testid="fork-name-problem">{nameProblem}</p>
      {/if}
      <p class="field-note" data-testid="fork-new-note">
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
        disabled={busy || moving}
        onchange={() => (choice = 'same')}
      />
      <span class="choice-label" data-testid="fork-same-warning"
        >Same worktree <Icon name="warning" size={12} /> both sessions edit the same files</span
      >
    </label>
  </fieldset>

  <label class="field">
    <span class="field-label">Host · agent</span>
    <span class="row">
      <select
        data-testid="fork-host"
        value={targetHost ?? ''}
        disabled={busy}
        onchange={(e) => pickHost((e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">{source?.host_alias ?? 'this host'} (this session's)</option>
        {#each otherHosts as h (h)}<option value={h}>{h}</option>{/each}
      </select>
      <span class="field-note">Claude Code</span>
    </span>
    {#if moving}
      <p class="field-note" data-testid="fork-move-note">
        Forks here, then moves the new session and its worktree to {targetHost}.
      </p>
    {/if}
  </label>

  {#if busy}
    <!-- Redesign step 5.13: forking is merging work, so the manual's Liquid
         orbit, inline where the new session will appear. -->
    <div class="merging" data-testid="fork-merging">
      <Loader name="liquid-orbit" size={56} label={moving ? 'Forking and moving' : 'Forking'} />
      <span>{moving ? `Copying the conversation, then moving it to ${targetHost}` : 'Copying the conversation into a new session'}</span>
    </div>
  {/if}
  {#if notice && notice !== 'forked'}
    <p class="notice" data-testid="fork-notice">{notice}</p>
  {/if}
</DialogSheet>

<style>
  .merging {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .choices {
    border: none;
    padding: 0;
    margin: 0;
  }
  .choice {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-1) 0;
    cursor: pointer;
  }
  .choice-label {
    line-height: 1.3;
  }
  .recommended {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .new-worktree-fields {
    margin: 0 0 var(--space-1) 1.5rem;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .problem {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--danger);
  }
  .notice {
    margin: 0;
    font-size: var(--text-sm);
  }
</style>
