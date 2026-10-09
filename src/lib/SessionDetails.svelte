<script lang="ts">
  import { tick, untrack } from 'svelte';
  import {
    sessions,
    hasNoPane,
    isInactiveAgent,
    dismissAgentSession,
    type SessionRow,
    type SafeKillInspection,
  } from './sessions';
  import { formatCostMicros, formatTokens, sessionUsageTokens } from './sessions';
  import {
    killSession,
    restartSession,
    repairSession,
    recreateSession,
    safeKillSession,
    inspectSafeKill,
    discardKillSession,
  } from './sessions';
  import { canMoveSession, moveBlockedReason } from './moveEligibility';
  import { transferSheetFor, startMove, adoptPartial, adoptWait } from './moves';
  import { moveOrigin, unresolvedPartial, unresolvedWait, type SessionEvent } from './timeline';
  import { projectById } from './projects';
  import { selectSession, selectSessionExplicitly, clearSelection } from './selection';
  import { hostByAlias } from './hosts';
  import { accountByUuid, accountEmailTier, type AccountRow } from './accounts';
  import { timeAgo } from './session_status';
  import { applySessionRename, renameKeyHandler } from './session_rename';
  import PromptComposer from './PromptComposer.svelte';
  import WatchSummary from './WatchSummary.svelte';
  import { uiLayout } from './prefs';
  import ReviewDialog from './ReviewDialog.svelte';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import KillDialog from './KillDialog.svelte';
  import TasksPanel from './TasksPanel.svelte';
  import TicketCard from './TicketCard.svelte';
  import LocalWorkspaceCard from './LocalWorkspaceCard.svelte';
  import PrResult from './PrResult.svelte';
  import { assessRow, hasReading } from './evidence';
  import SessionTasks from './SessionTasks.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { proposalFor } from './proposals';
  import Timeline from './Timeline.svelte';
  import { push, pushError } from './toasts';
  import { copyText } from './clipboard';
  import {
    ciStatusColor,
    ciStatusLabel,
    claudeStatusColor,
    claudeStatusLabel,
    contextColor,
    contextLevel,
    formatElapsed,
    sessionStart,
    stuckStatus,
    sessionStatusWord,
    STUCK_COLOR,
  } from './attention';
  import { hubStatus, hubBlock, hubActionBlocked } from './hub';
  import { accessOf } from './access';
  import { shareSheetFor, sessionBlocked } from './share';
  import { hubConnection } from './hub_connection';
  import { sessionActionRequest, takeSessionAction, type SessionActionId } from './session_actions';

  let { session }: { session: SessionRow } = $props();

  // None of these three has a hub tool (`commands/sessions.rs`): the
  // pre-flight git inspect and the one-step discard-and-kill both act over
  // this machine's SSH connection, and dismiss_agent_session's job is done
  // instead by the hub's own kill_session.
  // Every one of these composes TWO predicates since multi-user M1: the hub's
  // (`hubBlock` / `hubActionBlocked`, which take an action name and no session)
  // and this client's access to THIS row (`$sessionBlocked`). They are composed
  // with `??` rather than merged into one call, because a predicate that took
  // both would let a call site pass the action and forget the session and still
  // read as allowed. The hub's half wins when both answer: "a client never
  // administers the fleet" is true of every session, and so the more useful
  // sentence than "this one is not yours". Read through the `$sessionBlocked`
  // STORE, not the bare function, so a revoke or a narrow re-disables these
  // without a re-list — a grant change moves no column on the row.
  const inspectSafeKillBlocked = $derived(
    hubBlock('inspect_safe_kill', $hubStatus) ?? $sessionBlocked(session, 'inspect_safe_kill'),
  );
  const discardKillBlocked = $derived(
    hubBlock('discard_kill_session', $hubStatus) ?? $sessionBlocked(session, 'discard_kill_session'),
  );
  const dismissAgentBlocked = $derived(
    hubBlock('dismiss_agent_session', $hubStatus) ??
      $sessionBlocked(session, 'dismiss_agent_session'),
  );

  // These route to the hub, so they stay enabled on a hub client — but only
  // while the live connection to it is up (`hubActionBlocked`).
  const killBlocked = $derived(
    hubActionBlocked('kill_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'kill_session'),
  );
  const restartBlocked = $derived(
    hubActionBlocked('restart_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'restart_session'),
  );
  const recreateBlocked = $derived(
    hubActionBlocked('recreate_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'recreate_session'),
  );
  const moveBlocked = $derived(
    moveBlockedReason($hubStatus, $hubConnection) ?? $sessionBlocked(session, 'move_session'),
  );
  const repairBlocked = $derived(
    hubActionBlocked('repair_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'repair_session'),
  );
  const renameBlocked = $derived(
    hubActionBlocked('rename_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'rename_session'),
  );
  const setFriendlyNameBlocked = $derived(
    hubActionBlocked('set_friendly_name', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'set_friendly_name'),
  );
  const safeKillClaudeBlocked = $derived(
    hubActionBlocked('safe_kill_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'safe_kill_session'),
  );
  /** Send prompt and Review both act on the owner's session — one types into
   *  the pane (`drive`), one creates a session in the owner's worktree with a
   *  terminal of its own, which spec §4.3 puts in the `own` tier. */
  const sendPromptBlocked = $derived(
    hubActionBlocked('send_prompt', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'send_prompt'),
  );
  const reviewBlocked = $derived(
    hubActionBlocked('spawn_review', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'spawn_review'),
  );
  /** Share… — owner only, and the sheet says so again on the inside. Both
   *  halves since F3: `session_share` routes to the hub (T13), so a paired
   *  desktop whose link is down gets "try again once it's back" rather than a
   *  sheet whose every button then fails. */
  const shareBlocked = $derived(
    hubActionBlocked('session_share', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'session_share'),
  );

  // Look up the parent project (if any) for context.
  const parentProject = $derived(
    session.project_id === null
      ? null
      : ($projectById.get(session.project_id) ?? null),
  );

  const hostRow = $derived($hostByAlias.get(session.host_alias) ?? null);
  // A session under a login profile bills that profile's account, not the
  // host's (docs/accounts.md); it has none until the host reports the
  // profile logged in.
  const accountRow = $derived(
    session.claude_profile
      ? session.account_uuid
        ? ($accountByUuid.get(session.account_uuid) ?? null)
        : null
      : hostRow?.account_uuid
        ? ($accountByUuid.get(hostRow.account_uuid) ?? null)
        : null,
  );

  // Switch the session to another login: a restart that resumes the same
  // conversation under it ('' = the host's own login).
  const canSwitchLogin = $derived(!hasNoPane(session) && session.kind !== 'shell');
  const loginChoices = $derived.by(() => {
    const listed = hostRow?.claude_profiles ?? [];
    const current = session.claude_profile;
    return current && !listed.some((p) => p.name === current)
      ? [...listed, { name: current, account_uuid: null, email: null }]
      : listed;
  });
  let loginPick = $state<string | null>(null);
  const loginTarget = $derived(loginPick ?? session.claude_profile ?? '');
  let confirmingSwitch = $state(false);
  async function onSwitchLogin() {
    confirmingSwitch = false;
    if (restartBlocked !== null) return;
    const target = loginTarget;
    const r = await restartSession(session.host_alias, session.tmux_name, target);
    if (!r.ok) pushError(r.error, 'Switching the login failed');
    else {
      loginPick = null;
      push({ kind: 'info', message: `Resumed under ${target || 'the host login'}` });
    }
  }
  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accountByUuid.get(s.account_uuid) ?? null;
  }

  const related = $derived(
    session.project_id == null || session.worktree_key == null
      ? []
      : $sessions.filter(
          (s) =>
            s.id !== session.id &&
            s.project_id === session.project_id &&
            s.worktree_key === session.worktree_key,
        ),
  );

  // N1 (redesign 6.9): another of this person's sessions Jev says works on
  // the same thing, when it is not listed above already. New layout only;
  // nothing is stopped or merged.
  const relatedProposal = $derived($uiLayout === 'new' ? proposalFor(session, 'related_session') : null);
  const proposedRelated = $derived.by(() => {
    const m = /^s(\d+)$/.exec(relatedProposal?.value ?? '');
    if (!m) return null;
    const id = Number(m[1]);
    if (related.some((r) => r.id === id)) return null;
    return $sessions.find((s) => s.id === id && s.id !== session.id) ?? null;
  });

  // Local-only for v0.2 (Phase 4 will branch on host_alias for remote attach).
  const attachCommand = $derived(`tmux attach -t ${session.tmux_name}`);

  // Multi-user M1: "sharing never confers a terminal" (spec §4.3 invariant 4)
  // covers this section too, and it is the easiest one to miss, because
  // nothing here invokes anything — it hands over the incantation and the
  // tmux name, which §4.3 counts as content, and the person types it into
  // their own shell where no hub can refuse it. So the section is gated on
  // the same derived answer the terminal pane is, through `$accessOf` so a
  // revoke takes it away without a re-list.
  const detailsOwned = $derived($accessOf(session) === 'own');

  // Past a month the relative form stops being useful; show the date.
  function formatRelative(unix: number): string {
    const ageSec = Math.floor(Date.now() / 1000) - unix;
    if (ageSec >= 30 * 86400) return new Date(unix * 1000).toISOString().slice(0, 10);
    return timeAgo(unix);
  }

  let copied = $state(false);

  // Coarse clock for the elapsed / idle counters (a minute-level readout
  // does not need a per-second re-render).
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const ctxLevel = $derived(contextLevel(session.context_pct));

  // Inline editor state — same UX as the sidebar's. Double-clicking the
  // title edits the display label (empty clears it); "Rename tmux session"
  // renames tmux itself.
  let renaming: 'label' | 'tmux' | null = $state(null);
  let renameValue = $state('');
  // Enter commits and then the input unmounts, which can fire blur → a
  // second commit. Same synchronous guard as the sidebar.
  let committingRename = false;

  async function onCopy() {
    const ok = await copyText(attachCommand, (e) => {
      push({ kind: 'error', code: 'E_CLIPBOARD', message: `Copy failed: ${String(e)}` });
    });
    if (!ok) return;
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  let renameInput: HTMLInputElement | undefined = $state();

  async function beginEdit(mode: 'label' | 'tmux') {
    renaming = mode;
    renameValue = mode === 'label' ? (session.friendly_name ?? '') : session.tmux_name;
    await tick();
    renameInput?.focus();
    renameInput?.select();
  }

  const beginRename = () => beginEdit('tmux');
  const beginLabelEdit = () => beginEdit('label');

  async function commitRename() {
    if (!renaming || committingRename) return;
    committingRename = true;
    try {
      const outcome = await applySessionRename(session, renaming, renameValue);
      if (outcome.kind === 'error') return;
      if (outcome.kind === 'ok' && outcome.row) selectSession(outcome.row, { follow: true });
      renaming = null;
    } finally {
      committingRename = false;
    }
  }

  function cancelRename() {
    renaming = null;
  }

  const onRenameKey = renameKeyHandler(() => void commitRename(), cancelRename);

  // Inactive bg agent: drop the row. The backend emits `session:removed`,
  // which removes it from the store (and clears the selection).
  async function onRemoveFromList() {
    if (dismissAgentBlocked !== null) return;
    const r = await dismissAgentSession(session.id);
    if (!r.ok) pushError(r.error, 'Remove failed');
  }

  // Restart stops the running claude and loses whatever it was mid-way
  // through. Kill and Recreate beside it both confirm; this did not, and it
  // wears the same `↻` the app uses for a harmless Refresh.
  let confirmingRestart = $state(false);
  function askRestart() {
    confirmingRestart = true;
  }
  function cancelRestart() {
    confirmingRestart = false;
  }
  async function onRestart() {
    if (restartBlocked !== null) return;
    confirmingRestart = false;
    const r = await restartSession(session.host_alias, session.tmux_name);
    if (!r.ok) pushError(r.error, 'Restart failed');
  }

  // Make the worktree directory + tmux pane healthy again (deleted dir,
  // pruned registration, moved checkout, dead tmux). The backend emits the
  // row events; the toast just says what it did.
  let repairing = $state(false);
  // Same consequence as Restart, so the same guard: an explicit repair may
  // respawn a LIVE pane, killing whatever claude is doing in it. It used to be
  // one click, sitting between Restart and Recreate, which both confirm.
  let confirmingRepair = $state(false);
  function askRepair() {
    confirmingRepair = true;
  }
  function cancelRepair() {
    confirmingRepair = false;
  }
  async function onRepair() {
    if (repairBlocked !== null) return;
    confirmingRepair = false;
    if (repairing) return;
    repairing = true;
    // Explicit: the user asked, so this may unregister a stale entry, adopt a
    // moved checkout, recreate the branch and respawn a live pane.
    const r = await repairSession(session.id, { explicit: true });
    repairing = false;
    if (!r.ok) {
      pushError(r.error, 'Repair failed');
      return;
    }
    const rep = r.value;
    if (rep.actions.length === 0) {
      const notes = rep.warnings.length > 0 ? ` (${rep.warnings.join('; ')})` : '';
      push({ kind: 'success', message: `Workspace is healthy: ${rep.cwd}${notes}` });
      return;
    }
    const branch = rep.branch_source ? ` [branch: ${rep.branch_source}]` : '';
    push({ kind: 'success', message: `Repaired workspace: ${rep.actions.join('; ')}${branch}` });
    if (rep.tmux === 'created') {
      // A recreated tmux session needs a fresh attach (same tmux_name, so
      // the selection effect would not fire on its own).
      selectSession(null);
      await tick();
      selectSession(session, { follow: true });
    }
  }

  let composerOpen = $state(false);
  function openComposer() {
    composerOpen = true;
  }

  let reviewOpen = $state(false);

  const reviewedSource = $derived.by(() => {
    if (session.kind !== 'review' || session.reviews_session_id == null) return null;
    return $sessions.find((s) => s.id === session.reviews_session_id) ?? null;
  });

  const reviewsOfThis = $derived(
    $sessions.filter((s) => s.kind === 'review' && s.reviews_session_id === session.id),
  );

  let confirmingKill = $state(false);
  let confirmingSafeKill = $state(false);
  let confirmingRecreate = $state(false);

  // Safe-remove inspection state. `null` while loading; populated once the
  // pre-flight git inspect returns. `safe_to_remove` short-circuits the
  // Claude prompt entirely.
  let inspection: SafeKillInspection | null = $state(null);
  let inspectError: string | null = $state(null);
  let busy = $state(false);

  async function askSafeKill() {
    // Re-asked at the call, not only on the control (multi-user M1, F2b): the
    // confirmation these sit behind stays on screen, and a grant can be
    // narrowed while it does.
    if (inspectSafeKillBlocked !== null) return;
    inspection = null;
    inspectError = null;
    confirmingSafeKill = true;
    const r = await inspectSafeKill(session.host_alias, session.tmux_name);
    if (r.ok) {
      inspection = r.value;
    } else {
      inspectError = r.error.message;
    }
  }
  function cancelSafeKill() {
    if (busy) return;
    confirmingSafeKill = false;
    inspection = null;
    inspectError = null;
  }

  // "Let Claude commit it" path: current behavior — send the marker-baked
  // prompt and wait for the Stop hook to finalize.
  async function doSafeKillViaClaude() {
    if (safeKillClaudeBlocked !== null) return;
    busy = true;
    const r = await safeKillSession(session.host_alias, session.tmux_name);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Safe remove failed');
      return;
    }
    confirmingSafeKill = false;
    inspection = null;
  }

  // Clean+pushed fast path: backend just removes the worktree + kills tmux,
  // no Claude involvement. `force=false` so a surprise dirty file errors out.
  async function doDirectRemove() {
    if (discardKillBlocked !== null) return;
    busy = true;
    const r = await discardKillSession(session.host_alias, session.tmux_name, false);
    busy = false;
    if (r.ok) {
      confirmingSafeKill = false;
      inspection = null;
      clearSelection();
    } else {
      pushError(r.error, 'Remove failed');
    }
  }

  // Explicit discard: user has seen the dirty list / unpushed warning and
  // chose to drop the work anyway.
  async function doDiscardAndKill() {
    if (discardKillBlocked !== null) return;
    busy = true;
    const r = await discardKillSession(session.host_alias, session.tmux_name, true);
    busy = false;
    if (r.ok) {
      confirmingSafeKill = false;
      inspection = null;
      clearSelection();
    } else {
      pushError(r.error, 'Discard & kill failed');
    }
  }
  function askKill() {
    confirmingKill = true;
  }
  function cancelKill() {
    confirmingKill = false;
  }
  async function doKill() {
    if (killBlocked !== null) return;
    confirmingKill = false;
    const r = await killSession(session.host_alias, session.tmux_name);
    if (r.ok) {
      clearSelection();
    } else {
      pushError(r.error, 'Kill failed');
    }
  }

  function askRecreate() {
    confirmingRecreate = true;
  }

  function cancelRecreate() {
    confirmingRecreate = false;
  }

  // Move to host…: the app's one Transfer sheet does the work (TransferSheet
  // + moves.ts); this button and the terminal header's chip both open it.
  const canMove = $derived(canMoveSession(session));

  function openMove() {
    transferSheetFor.set(session.id);
  }

  // Share… : the app's one Share sheet does the work (ShareSheet + share.ts),
  // exactly the way Move to host… opens the Transfer sheet. This button only
  // opens it; the recipient, the level, the live grant list and the two
  // consequences a sharer is deciding without being told all live there.
  function openShare() {
    shareSheetFor.set(session.id);
  }

  // Recovery, long after the transfer: the durable record is the session's
  // own timeline, handed up from Timeline's onEvents rather than fetched a
  // second time here (see Timeline.svelte). `moveOrigin`/`unresolvedPartial`
  // are pure over that same event list.
  let timelineEvents = $state<SessionEvent[]>([]);
  /**
   * Where this session came from — unless that host is still running this
   * very conversation, in which case there is no trip back to offer.
   *
   * Two shapes, one check. A `keep_source` move leaves the origin running
   * the same `claude_session_id` in the same worktree, so a move "back"
   * would aim the transfer at that live session's own worktree (the engine
   * refuses it with `E_INVALID_STATE`; the panel does not offer it). And
   * `session_moved` is recorded on BOTH rows, so the SOURCE row's own panel
   * reads an event naming the host it is already on — that row is itself the
   * live session the search finds, which is why nothing here excludes it.
   */
  const moveBackOrigin = $derived.by(() => {
    const origin = moveOrigin(timelineEvents);
    if (!origin) return null;
    const conversation = origin.claudeSessionId ?? session.claude_session_id;
    if (conversation === null) return origin;
    const stillThere = $sessions.some(
      (s) =>
        s.host_alias === origin.fromHost &&
        s.status === 'running' &&
        s.claude_session_id === conversation,
    );
    return stillThere ? null : origin;
  });
  const unresolvedMove = $derived(unresolvedPartial(timelineEvents));
  /** A pending wait for a busy source to go idle, recorded on this session's
   *  own timeline — the wait's `session_move_waiting` is only ever written
   *  to the source's own row, so this only ever finds one on the source's
   *  own panel, the same way `unresolvedMove` only ever names a partial on
   *  a row involved in it. */
  const unresolvedWaitRec = $derived(unresolvedWait(timelineEvents));

  function openMoveBack() {
    if (!moveBackOrigin || moveBlocked !== null) return;
    startMove(session, moveBackOrigin.fromHost, { keepSource: false });
  }

  // Finish/Undo are destructive (they kill the source or the new session) and
  // their confirmations + refusal text live in the Transfer sheet alone — this
  // panel only opens it, never calls resolveMoveRun itself. After a restart
  // there is no in-memory run to render, so adoptPartial rebuilds one from the
  // recorded event before the sheet opens, keyed the same way it is (source
  // id when known, else target id) so the sheet opens on the run it just made.
  function openFinishOrUndo() {
    if (!unresolvedMove || moveBlocked !== null) return;
    adoptPartial(unresolvedMove, session.tmux_name);
    transferSheetFor.set(unresolvedMove.sourceSessionId ?? session.id);
  }

  /** Same recovery shape as `openFinishOrUndo`, for a wait instead of a
   *  partial: `adoptWait` rebuilds the run from the recorded event (a no-op
   *  if a live one already exists), then the sheet opens on it — Cancel and
   *  the reason it ended live there, not in this panel. */
  function openWait() {
    if (!unresolvedWaitRec || moveBlocked !== null) return;
    adoptWait(unresolvedWaitRec, session.tmux_name);
    transferSheetFor.set(unresolvedWaitRec.sessionId);
  }

  async function doRecreate() {
    if (recreateBlocked !== null) return;
    confirmingRecreate = false;
    const r = await recreateSession(session.id);
    if (!r.ok) {
      pushError(r.error, 'Recreate failed');
      return;
    }
    // kill-session severed the PTY; same tmux_name won't auto-reopen. This
    // panel shows the selected session, so force a re-attach.
    selectSession(null);
    await tick();
    selectSession(r.value, { follow: true });
  }
  // A row's ⋯ menu or right-click asked for an action on this session
  // (redesign step 3.10, `session_actions.ts`): run it as this pane's own
  // button would, with the same gate, confirm and dialog.
  const rowActions: Record<SessionActionId, { blocked: () => string | null; run: () => void }> = {
    label: { blocked: () => setFriendlyNameBlocked, run: beginLabelEdit },
    rename: { blocked: () => renameBlocked, run: beginRename },
    restart: { blocked: () => restartBlocked, run: askRestart },
    repair: { blocked: () => (repairing ? 'Repairing…' : repairBlocked), run: askRepair },
    send_prompt: { blocked: () => sendPromptBlocked, run: openComposer },
    review: { blocked: () => reviewBlocked, run: () => (reviewOpen = true) },
    recreate: { blocked: () => recreateBlocked, run: askRecreate },
    move: { blocked: () => moveBlocked, run: openMove },
    share: { blocked: () => shareBlocked, run: openShare },
    remove_from_list: { blocked: () => dismissAgentBlocked, run: () => void onRemoveFromList() },
    safe_remove: { blocked: () => inspectSafeKillBlocked, run: () => void askSafeKill() },
    kill: { blocked: () => killBlocked, run: askKill },
  };
  $effect(() => {
    const r = $sessionActionRequest;
    if (!r || r.sessionId !== session.id) return;
    untrack(() => {
      const taken = takeSessionAction(session.id);
      if (!taken || taken.action === 'details') return;
      const a = rowActions[taken.action];
      if (a.blocked() === null) a.run();
    });
  });
</script>

<article class="details" data-testid="session-details">
  <header class="header">
    {#if renaming}
      <input
        bind:this={renameInput}
        class="title-input"
        data-testid={renaming === 'label' ? 'details-label' : 'details-rename'}
        aria-label={renaming === 'label'
          ? `Label for ${session.tmux_name} (empty clears it)`
          : `New tmux session name for ${session.tmux_name}`}
        placeholder={renaming === 'label' ? session.tmux_name : undefined}
        bind:value={renameValue}
        onkeydown={onRenameKey}
        onblur={commitRename}
      />
    {:else}
      <h2
        class="title"
        ondblclick={beginLabelEdit}
        title="Double-click to edit the label"
      >{session.tmux_name}</h2>
    {/if}
    {#if session.friendly_name && renaming !== 'label'}
      <p class="friendly" data-testid="details-friendly-name">{session.friendly_name}</p>
    {/if}
    <div class="sub">
      <span class="host">{session.host_alias}</span>
      <span class="status status-{session.status}">{session.status}</span>
      {#if session.stuck_kind}
        <span
          class="chip stuck-chip"
          data-testid="details-stuck"
          style="background: color-mix(in srgb, {STUCK_COLOR} 13%, transparent); color: {STUCK_COLOR}; border-color: color-mix(in srgb, {STUCK_COLOR} 40%, transparent);"
          title={session.current_activity ?? undefined}
        >{stuckStatus(session.stuck_kind)}{#if session.stuck_since !== null} · {formatElapsed(session.stuck_since, nowSec)}{/if}</span>
      {:else if session.claude_status}
        <span
          class="chip claude-chip"
          data-testid="details-claude-status"
          style="background: color-mix(in srgb, {claudeStatusColor(session.claude_status)} 13%, transparent); color: {claudeStatusColor(session.claude_status)}; border-color: color-mix(in srgb, {claudeStatusColor(session.claude_status)} 27%, transparent);"
          title={session.current_activity ?? undefined}
        >{claudeStatusLabel(session.claude_status)}</span>
      {/if}
      {#if ctxLevel !== null && session.context_pct !== null}
        <span
          class="chip"
          data-testid="details-context"
          data-level={ctxLevel}
          style="color: {contextColor(ctxLevel)}; border-color: {contextColor(ctxLevel)};"
          title="Context window used"
        >ctx {Math.round(session.context_pct)}%</span>
      {/if}
    </div>
  </header>

  {#if $uiLayout === 'new'}
    <!-- Orbit Fleet 11.11: the watcher's summary tops the facts too. -->
    <WatchSummary {session} />
  {/if}

  <dl class="meta">
    <dt>Host</dt>
    <dd data-testid="session-host">{session.host_alias}</dd>

    <dt>Account</dt>
    <dd data-testid="session-account">{accountEmailTier(accountRow)}</dd>

    {#if canSwitchLogin}
      <dt>Login</dt>
      <dd class="login" data-testid="session-login">
        <select
          aria-label="Claude login"
          data-testid="session-login-pick"
          value={loginTarget}
          onchange={(e) => (loginPick = (e.currentTarget as HTMLSelectElement).value)}
          disabled={restartBlocked !== null}
          title="The Claude login this session bills: the host's own, or a login profile (~/.claude-profiles/<name>)"
        >
          <option value="">Host login</option>
          {#each loginChoices as p (p.name)}
            <option value={p.name}>{p.name}{p.email ? ` (${p.email})` : p.account_uuid ? '' : ' (not logged in)'}</option>
          {/each}
        </select>
        {#if loginTarget !== (session.claude_profile ?? '')}
          <button
            data-testid="session-login-switch"
            onclick={() => (confirmingSwitch = true)}
            disabled={restartBlocked !== null}
            title={restartBlocked ?? ''}
          >Switch…</button>
        {/if}
      </dd>
    {/if}

    <dt>Project</dt>
    <dd>
      {#if parentProject}
        {parentProject.project.owner}/{parentProject.project.repo}
      {:else}
        <span class="muted">unmapped (orphan)</span>
      {/if}
    </dd>

    <dt>Created</dt>
    <dd>{formatRelative(session.created_at)}</dd>

    <dt>Last activity</dt>
    <dd>{formatRelative(session.last_activity_at)}</dd>

    <dt>Elapsed</dt>
    <dd data-testid="details-elapsed" title={session.started_at === null ? 'since tmux created the session (fleet did not start it)' : 'since fleet started the session'}>
      {formatElapsed(sessionStart(session), nowSec)}
    </dd>

    {#if session.last_turn_at !== null}
      <dt>Last turn</dt>
      <dd data-testid="details-last-turn">{formatRelative(session.last_turn_at)}</dd>
    {/if}

    {#if session.last_prompt}
      <dt>Last prompt</dt>
      <dd class="last-prompt" data-testid="details-last-prompt">{session.last_prompt}</dd>
    {/if}

    {#if sessionUsageTokens(session) > 0}
      <dt>Usage</dt>
      <dd
        data-testid="details-usage"
        title="Estimated from the Claude Code transcript's token counts and a built-in per-model price table (override: usage.prices_json). Not a bill."
      >
        <span data-testid="details-cost">{#if (session.usage_cost_micros ?? 0) > 0}{formatCostMicros(session.usage_cost_micros)} estimated{:else}unpriced ({session.usage_model ?? 'unknown model'}){/if}</span>
        <span class="muted">· {formatTokens(session.usage_input_tokens)} in · {formatTokens(session.usage_output_tokens)} out · {formatTokens(session.usage_cache_write_tokens)} cache write · {formatTokens(session.usage_cache_read_tokens)} cache read{#if session.usage_model} · {session.usage_model}{/if}</span>
      </dd>
    {/if}

    {#if session.pr_url}
      <dt>Pull request</dt>
      <dd data-testid="details-pr">
        <a class="pr-link" href={session.pr_url} target="_blank" rel="noreferrer">{session.pr_url.replace(/^https:\/\/github\.com\//, '')}</a>
        {#if session.ci_status}
          <span
            class="chip"
            data-testid="details-ci"
            style="color: {ciStatusColor(session.ci_status)}; border-color: color-mix(in srgb, {ciStatusColor(session.ci_status)} 33%, transparent);"
            title="CI checks: {session.ci_status}"
          >{ciStatusLabel(session.ci_status)}</span>
        {/if}
      </dd>
      {#if hasReading(assessRow(session, nowSec))}
        <dt>Result</dt>
        <dd data-testid="details-pr-result"><PrResult {session} {nowSec} /></dd>
      {/if}
    {/if}

    {#if reviewedSource}
      <dt class="meta-label">Reviewing</dt>
      <dd>
        <button class="link" onclick={() => selectSessionExplicitly(reviewedSource)} data-testid="reviewing-link">
          {reviewedSource.tmux_name}
        </button>
      </dd>
    {/if}
  </dl>

  <TicketCard {session} />
  <SessionTasks {session} />
  <LocalWorkspaceCard {session} />

  {#if related.length > 0 || proposedRelated}
    <section class="related" data-testid="related-sessions">
      <h3>Related sessions ({related.length + (proposedRelated ? 1 : 0)})</h3>
      <ul class="related-list">
        {#if proposedRelated}
          {@const r = proposedRelated}
          <li class="proposed" data-testid="related-proposed">
            <button class="related-row" data-testid="related-proposed-row" onclick={() => selectSessionExplicitly(r)}>
              <span class="host-badge">[{r.host_alias}]</span>
              <span class="account">Same work?</span>
              <span class="status-word" data-status={r.status}>{sessionStatusWord(r)}</span>
              <span class="sess-name">{r.tmux_name}</span>
              <span class="age">{formatRelative(r.last_activity_at)}</span>
            </button>
            <ProposedBy proposal={relatedProposal} field="related_session" testid="related-proposed-by" />
          </li>
        {/if}
        {#each related as r (r.id)}
          <li>
            <button
              class="related-row"
              data-testid="related-row"
              onclick={() => selectSessionExplicitly(r)}
            >
              <span class="host-badge">[{r.host_alias}]</span>
              <span class="account">{accountEmailTier(accountForRow(r))}</span>
              <span class="status-word" data-status={r.status}>{sessionStatusWord(r)}</span>
              <span class="sess-name">{r.tmux_name}</span>
              <span class="age">{formatRelative(r.last_activity_at)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if reviewsOfThis.length > 0}
    <section class="related" data-testid="reviews-panel">
      <h3>Reviews ({reviewsOfThis.length})</h3>
      <ul class="related-list">
        {#each reviewsOfThis as r (r.id)}
          <li>
            <button
              class="related-row"
              data-testid="reviews-row"
              onclick={() => selectSessionExplicitly(r)}
            >
              <span class="host-badge">[{r.host_alias}]</span>
              <span class="account">{accountEmailTier(accountForRow(r))}</span>
              <span class="status-word" data-status={r.status}>{sessionStatusWord(r)}</span>
              <span class="sess-name">{r.tmux_name}</span>
              <span class="age">{formatRelative(r.last_activity_at)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <TasksPanel sessionId={session.id} />

  <Timeline
    sessionId={session.id}
    refreshKey={`${session.turn_seq}|${session.status}|${session.claude_status}|${session.stuck_kind}|${session.last_prompt}|${session.safe_kill_state}`}
    onEvents={(e) => (timelineEvents = e)}
  />

  {#if !hasNoPane(session) && detailsOwned}
    <section class="block">
      <h3>Attach from another terminal</h3>
      <div class="cmd-row">
        <code class="cmd" data-testid="attach-command">{attachCommand}</code>
        <button class="copy" onclick={onCopy} data-testid="copy-attach">
          {copied ? '✓ copied' : 'copy'}
        </button>
      </div>
    </section>
  {/if}

  <!-- Redesign 1.5, the action hierarchy: one primary (Send prompt), three
       quick actions, and the rest behind ⋯ with the destructive ones last,
       each behind its own confirm. Nothing is gone: every action that was a
       button here is still a button, one click further at most. A pending
       move's own controls stay in view, since they are the next step. -->
  <section class="block actions" data-testid="details-actions">
    {#if session.kind !== 'external'}
      {#if session.kind !== 'shell'}
        <button
          class="btn btn--primary"
          onclick={openComposer}
          disabled={sendPromptBlocked !== null}
          title={sendPromptBlocked ?? ''}
          data-testid="send-prompt-from-details"
    >
          → Send prompt…
        </button>
      {/if}
      <button
        class="btn btn--quiet is-bounded"
        onclick={() => (reviewOpen = true)}
        disabled={reviewBlocked !== null}
        title={reviewBlocked ?? ''}
        data-testid="open-review"
  >
        Review…
      </button>
      <button
        class="btn btn--quiet is-bounded"
        onclick={openShare}
        disabled={shareBlocked !== null}
        title={shareBlocked ?? 'Share this session with one person — watch or drive, revocable, and never a terminal'}
        data-testid="share-from-details"
  >
        Share…
      </button>
      {#if moveBackOrigin}
        <button
          class="btn btn--quiet is-bounded"
          onclick={openMoveBack}
          disabled={moveBlocked !== null}
          title={moveBlocked ?? 'Move this session back to the host it came from'}
          data-testid="details-move-back"
    >
          ⇄ Move back to {moveBackOrigin.fromHost}
        </button>
      {/if}
      {#if unresolvedMove}
        <button
          class="btn btn--quiet is-bounded"
          onclick={openFinishOrUndo}
          disabled={moveBlocked !== null}
          title={moveBlocked ?? 'Open the Transfer sheet to finish this move'}
          data-testid="details-finish-move"
    >
          Finish the move to {unresolvedMove.toHost}…
        </button>
        <button
          class="btn btn--quiet is-bounded"
          onclick={openFinishOrUndo}
          disabled={moveBlocked !== null}
          title={moveBlocked ?? 'Open the Transfer sheet to undo this move'}
          data-testid="details-undo-move"
    >
          Undo the move…
        </button>
      {/if}
      {#if unresolvedWaitRec}
        <button
          class="btn btn--quiet is-bounded"
          onclick={openWait}
          disabled={moveBlocked !== null}
          title={moveBlocked ?? 'Open the Transfer sheet for this pending move'}
          data-testid="details-resume-wait"
    >
          ⇄ Waiting to move to {unresolvedWaitRec.toHost}…
        </button>
      {/if}
    {/if}
    <details class="more" data-testid="details-more">
      <summary class="btn btn--quiet is-bounded" aria-label="More actions" title="More actions">⋯</summary>
      <div class="more-menu">
        <button
          class="menu-item"
          onclick={beginLabelEdit}
          disabled={setFriendlyNameBlocked !== null}
          title={setFriendlyNameBlocked ?? ''}
          data-testid="label-from-details"
    >
          Rename
        </button>
        <!-- An external row runs outside fleet: the label (local fleet
             metadata) is the only thing fleet can change about it. -->
        {#if session.kind !== 'external'}
          <button
            class="menu-item"
            onclick={beginRename}
            disabled={renameBlocked !== null}
            title={renameBlocked ?? ''}
            data-testid="rename-from-details"
      >
            Rename tmux session
          </button>
          <button
            class="menu-item"
            onclick={askRestart}
            disabled={restartBlocked !== null}
            title={restartBlocked ?? ''}
            data-testid="restart-from-details"
      >
            ↻ Restart…
          </button>
          {#if !hasNoPane(session) && session.project_id !== null}
            <button
              class="menu-item"
              onclick={askRepair}
              disabled={repairing || repairBlocked !== null}
              title={repairBlocked ?? 'Recreate a deleted worktree directory, re-register it with git, and respawn the pane in it'}
              data-testid="repair-from-details"
        >
              Repair workspace…
            </button>
          {/if}
          <button
            class="menu-item"
            onclick={askRecreate}
            disabled={recreateBlocked !== null}
            title={recreateBlocked ?? ''}
            data-testid="recreate-from-details"
      >
            Recreate…
          </button>
          {#if canMove}
            <button
              class="menu-item"
              onclick={openMove}
              disabled={moveBlocked !== null}
              title={moveBlocked ?? 'Continue this conversation on another host: same branch, same Claude session'}
              data-testid="move-from-details"
        >
              ⇄ Move to host…
            </button>
          {/if}
          {#if isInactiveAgent(session)}
            <button
              class="menu-item"
              onclick={onRemoveFromList}
              disabled={dismissAgentBlocked !== null}
              title={dismissAgentBlocked ?? 'Hide this inactive agent until it becomes active again'}
              data-testid="remove-from-list-details"
        >
              Remove from list
            </button>
          {/if}
          <!-- Destructive, last, each behind its confirm. An inactive
               agent's daemon is gone: Remove from list (above) is its only
               removal action. -->
          {#if !isInactiveAgent(session)}
            <hr class="more-sep" />
            {#if session.kind !== 'shell' && session.status === 'running' && session.safe_kill_state !== 'requested'}
              <button
                class="menu-item"
                onclick={askSafeKill}
                disabled={inspectSafeKillBlocked !== null}
                title={inspectSafeKillBlocked ?? ''}
                data-testid="safe-kill-from-details"
          >
                Safe remove…
              </button>
            {/if}
            <button
              class="menu-item danger"
              onclick={askKill}
              disabled={killBlocked !== null}
              title={killBlocked ?? ''}
              data-testid="kill-from-details"
        >
              Kill session…
            </button>
          {/if}
        {/if}
      </div>
    </details>
  </section>

  {#if session.safe_kill_state === 'requested'}
    <p class="safe-kill-pill pending" data-testid="safe-kill-pending">
      Safe-remove in progress: asked Claude to commit + push. Will delete the
      worktree and kill the session once it reports back.
    </p>
  {:else if session.safe_kill_state === 'failed'}
    <p class="safe-kill-pill failed" data-testid="safe-kill-failed">
      Safe-remove failed: {session.safe_kill_detail ?? 'no reason given'}.
      Resolve in the session, then retry, or use <strong>Kill session…</strong>.
    </p>
  {:else if session.safe_kill_state === 'ready'}
    <p class="safe-kill-pill ready" data-testid="safe-kill-ready">
      Safe-remove ready — finalizing.
    </p>
  {/if}
</article>

{#if composerOpen}
  <PromptComposer source={session} onClose={() => (composerOpen = false)} />
{/if}

{#if reviewOpen}
  <ReviewDialog source={session} onClose={() => (reviewOpen = false)} />
{/if}

{#if confirmingRestart}
  <ConfirmDialog
    title="Restart claude?"
    confirmLabel="Restart"
    danger
    onconfirm={onRestart}
    oncancel={cancelRestart}
    confirmTestId="confirm-restart-details"
  >
    This stops the claude process in <code>{session.tmux_name}</code> on
    <code>{session.host_alias}</code> and starts a fresh one. Anything it is working
    on right now is lost; the tmux session and the worktree are kept. Continue?
  </ConfirmDialog>
{/if}

{#if confirmingSwitch}
  <ConfirmDialog
    title="Switch login?"
    confirmLabel="Switch"
    danger
    onconfirm={onSwitchLogin}
    oncancel={() => (confirmingSwitch = false)}
    confirmTestId="confirm-login-switch"
  >
    This restarts claude in <code>{session.tmux_name}</code> and resumes the same
    conversation under <code>{loginTarget || 'the host login'}</code>. Anything it is
    working on right now is lost. A profile that is not logged in yet asks for
    <code>/login</code> in the session.
  </ConfirmDialog>
{/if}

{#if confirmingRepair}
  <ConfirmDialog
    title="Repair workspace?"
    confirmLabel="Repair"
    danger
    busy={repairing}
    onconfirm={onRepair}
    oncancel={cancelRepair}
    confirmTestId="confirm-repair-details"
  >
    This may re-register the worktree for <code>{session.tmux_name}</code> on
    <code>{session.host_alias}</code>, recreate its branch and respawn the pane —
    which stops the claude running in it, losing whatever it is working on. The
    worktree's files are kept. Continue?
  </ConfirmDialog>
{/if}

{#if confirmingKill}
  <KillDialog
    targets={[session]}
    onkill={doKill}
    oncleaned={(removed) => {
      confirmingKill = false;
      if (removed.length > 0) clearSelection();
    }}
    oncancel={cancelKill}
    confirmTestId="confirm-kill-details"
  />
{/if}

{#if confirmingSafeKill}
  <Modal label="Safe remove {session.tmux_name}" onclose={cancelSafeKill} width="480px" testid="safe-kill-dialog">
    <div class="confirm wide">
      <h3>Safe remove <code>{session.tmux_name}</code>?</h3>

      {#if inspection === null && inspectError === null}
        <p class="muted">Inspecting worktree…</p>
        <div class="confirm-actions">
          <button onclick={cancelSafeKill}>Cancel</button>
        </div>
      {:else if inspectError}
        <p>
          Couldn't inspect the worktree: <code>{inspectError}</code>. You can
          still ask Claude to persist the work safely.
        </p>
        <div class="confirm-actions">
          <button onclick={cancelSafeKill}>Cancel</button>
          <button
            disabled={busy || safeKillClaudeBlocked !== null}
            title={safeKillClaudeBlocked ?? ''}
            onclick={doSafeKillViaClaude}
            data-testid="confirm-safe-kill-claude"
          >Ask Claude to commit + push</button>
        </div>
      {:else if inspection && inspection.safe_to_remove}
        <p>
          Worktree is clean and branch
          <code>{inspection.branch ?? '(detached)'}</code> is up-to-date with
          <code>{inspection.upstream ?? 'origin'}</code>. Safe to remove
          immediately.
        </p>
        <div class="confirm-actions">
          <button disabled={busy} onclick={cancelSafeKill}>Cancel</button>
          <button
            class="primary"
            disabled={busy || discardKillBlocked !== null}
            title={discardKillBlocked ?? ''}
            onclick={doDirectRemove}
            data-testid="confirm-safe-kill-direct"
          >Remove worktree + kill</button>
        </div>
      {:else if inspection}
        {#if !inspection.has_worktree}
          <p>
            This session has no tracked worktree. Asking Claude to commit and
            push will surface any unsaved work; the session is then killed.
          </p>
        {:else}
          <div class="inspect-box">
            <p class="inspect-line">
              Branch: <code>{inspection.branch ?? '(detached)'}</code>
              {#if inspection.upstream}
                → <code>{inspection.upstream}</code>
              {:else}
                <span class="muted">(no upstream — not pushed)</span>
              {/if}
            </p>
            {#if inspection.upstream && inspection.unpushed_commits > 0}
              <p class="inspect-line warn">
                {inspection.unpushed_commits} unpushed commit{inspection.unpushed_commits === 1 ? '' : 's'}
                on this branch.
              </p>
            {/if}
            {#if inspection.dirty_files.length > 0}
              <p class="inspect-line warn">
                {inspection.dirty_files.length} uncommitted file{inspection.dirty_files.length === 1 ? '' : 's'}:
              </p>
              <ul class="dirty-list" data-testid="dirty-files">
                {#each inspection.dirty_files.slice(0, 20) as f (f.path)}
                  <li>
                    <code class="status-code">{f.status}</code>
                    <span>{f.path}</span>
                  </li>
                {/each}
                {#if inspection.dirty_files.length > 20}
                  <li class="muted">… and {inspection.dirty_files.length - 20} more</li>
                {/if}
              </ul>
            {/if}
          </div>
          <p class="muted small">
            "Let Claude commit it" sends a prompt asking Claude to commit + push
            (to <code>main</code> or a PR) before fleet removes the worktree.
            "Discard &amp; kill" force-removes the worktree and kills the
            session — local-only changes will be lost.
          </p>
        {/if}
        <div class="confirm-actions">
          <button disabled={busy} onclick={cancelSafeKill}>Cancel</button>
          <button
            disabled={busy || safeKillClaudeBlocked !== null}
            title={safeKillClaudeBlocked ?? ''}
            onclick={doSafeKillViaClaude}
            data-testid="confirm-safe-kill-claude"
          >Let Claude commit it</button>
          <button
            class="danger"
            disabled={busy || discardKillBlocked !== null}
            title={discardKillBlocked ?? ''}
            onclick={doDiscardAndKill}
            data-testid="confirm-safe-kill-discard"
          >Discard &amp; kill</button>
        </div>
      {/if}
    </div>
  </Modal>
{/if}

{#if confirmingRecreate}
  <ConfirmDialog
    title="Recreate session?"
    confirmLabel="Recreate"
    danger
    onconfirm={doRecreate}
    oncancel={cancelRecreate}
    confirmTestId="confirm-recreate-details"
  >
    This kills the tmux session <code>{session.tmux_name}</code> on
    <code>{session.host_alias}</code> and the running claude state inside it, then
    starts a fresh session in the same worktree. Continue?
  </ConfirmDialog>
{/if}

<style>
  .details {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    color: var(--fg);
  }
  .header { display: flex; flex-direction: column; gap: 0.3rem; }
  .title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
    font-family: var(--font-mono);
    cursor: text;
    padding: 0.05rem 0;
    border-radius: var(--radius-xs);
  }
  .title:hover { background: var(--bg-pane); }
  .title-input {
    font-size: var(--text-lg);
    font-weight: 600;
    font-family: var(--font-mono);
    padding: 0.1rem 0.3rem;
    border: 1px solid var(--accent);
    background: var(--bg);
    color: var(--fg);
    border-radius: var(--radius-sm);
    outline: none;
  }
  .sub { display: flex; gap: 0.5rem; align-items: center; font-size: var(--text-2xs); flex-wrap: wrap; }
  .friendly { margin: 0; font-size: var(--text-xs); color: var(--fg-muted); }
  .chip {
    padding: 0.1rem 0.4rem;
    border-radius: var(--radius-pill);
    border: 1px solid;
    font-size: var(--text-2xs);
    white-space: nowrap;
  }
  .last-prompt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    max-height: 6rem;
    overflow: auto;
  }
  .pr-link { color: var(--accent); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .host {
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.1rem 0.4rem;
    border-radius: var(--radius-pill);
  }
  .status {
    padding: 0.1rem 0.4rem;
    border-radius: var(--radius-pill);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: var(--text-2xs);
  }
  .status-running { background: var(--done-soft); color: var(--status-done); }
  .status-frozen { background: var(--accent-soft); color: var(--status-working); }
  .status-orphan { background: var(--failed-soft); color: var(--status-failed); }

  .meta {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 0.75rem;
    margin: 0;
  }
  .meta dt {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .meta dd { margin: 0; font-size: var(--text-sm); }
  .muted { color: var(--fg-muted); font-style: italic; }

  .block h3 {
    margin: 0 0 0.4rem 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .cmd-row {
    display: flex;
    align-items: stretch;
    gap: 0.4rem;
  }
  .cmd {
    flex: 1;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    padding: 0.4rem 0.6rem;
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    overflow-x: auto;
    white-space: nowrap;
  }
  .copy {
    font-size: var(--text-2xs);
    padding: 0.4rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
    min-width: 4rem;
  }
  .copy:hover { border-color: var(--accent); }

  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: flex-start; }
  /* ⋯: everything that is not one of the three quick actions. */
  .more { position: relative; }
  .more > summary { list-style: none; }
  .more > summary::-webkit-details-marker { display: none; }
  .more-menu {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 4px;
    padding: 4px;
    min-width: 13rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-raise);
  }
  .menu-item {
    text-align: left;
    font: inherit;
    font-size: var(--text-xs);
    padding: 0.3rem 0.6rem;
    min-height: 24px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .menu-item:hover:not(:disabled) { background: var(--control-bg-hover); }
  .menu-item:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: -2px; }
  .menu-item.danger { color: var(--danger); }
  .menu-item.danger:hover:not(:disabled) { background: color-mix(in srgb, var(--danger) 10%, transparent); }
  /* One disabled look, distinct from a quiet button: dimmed, no hover, and
     the reason in the title. */
  .menu-item:disabled { opacity: 0.55; cursor: default; }
  .more-sep { width: 100%; border: none; border-top: 1px solid var(--border); margin: 3px 0; }
  .danger {
    font-size: var(--text-xs);
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--danger);
    background: transparent;
    color: var(--danger);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .danger:hover { background: color-mix(in srgb, var(--danger) 10%, transparent); }

  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }

  .safe-kill-pill {
    margin: 0;
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-md);
    font-size: var(--text-2xs);
    line-height: 1.35;
  }
  .safe-kill-pill.pending {
    background: var(--accent-soft);
    color: var(--status-working);
    border: 1px solid color-mix(in srgb, var(--status-working) 40%, transparent);
  }
  .safe-kill-pill.failed {
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
  }
  .safe-kill-pill.ready {
    background: var(--done-soft);
    color: var(--status-done);
    border: 1px solid color-mix(in srgb, var(--status-done) 40%, transparent);
  }

  .link {
    background: transparent;
    border: none;
    padding: 0;
    color: var(--accent);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .link:hover { opacity: 0.8; }

  /* Safe-remove body (lives inside Modal, which owns the box chrome). */
  .confirm {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .confirm h3 { margin: 0; font-size: var(--text-sm); }
  .confirm p { margin: 0; font-size: var(--text-xs); color: var(--fg-muted); line-height: 1.4; }
  .confirm code {
    font-family: var(--font-mono);
    background: var(--bg-pane);
    padding: 0.1rem 0.3rem;
    border-radius: var(--radius-xs);
    color: var(--fg);
  }
  .confirm-actions { display: flex; gap: 0.4rem; justify-content: flex-end; flex-wrap: wrap; }
  .confirm .primary {
    background: var(--accent);
    color: var(--accent-fg);
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.7rem;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .confirm .primary:disabled { opacity: 0.6; cursor: not-allowed; }
  .confirm button:disabled { opacity: 0.6; cursor: not-allowed; }
  .inspect-box {
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.5rem 0.7rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .inspect-line { margin: 0; font-size: var(--text-2xs); color: var(--fg); }
  .inspect-line.warn { color: var(--status-waiting); }
  .dirty-list {
    margin: 0.2rem 0 0 0;
    padding: 0;
    list-style: none;
    max-height: 9rem;
    overflow-y: auto;
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
  }
  .dirty-list li {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    padding: 0.05rem 0;
  }
  .status-code {
    color: var(--status-waiting);
    width: 2ch;
    flex: 0 0 auto;
    white-space: pre;
  }
  .small { font-size: var(--text-2xs); }
  .confirm-actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .confirm-actions button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }
  .confirm-actions button.danger:hover { background: color-mix(in srgb, var(--danger) 12%, transparent); }

  .related {
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    margin-top: 0.6rem;
  }
  .related h3 {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 0.4rem 0;
  }
  .related-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .related-row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.35rem 0.5rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: var(--text-2xs);
  }
  .related-row:hover {
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .related .host-badge {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
  }
  .related .account {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .related .sess-name {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
  }
  .related .age {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  /* The status as a word (7.2): the bare dot it replaces had no style, so
     it showed nothing and said nothing. */
  .related .status-word {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
