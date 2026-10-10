<script lang="ts">
  import { onDestroy, tick, untrack } from 'svelte';
  import {
    sessions,
    hasNoPane,
    isInactiveAgent,
    dismissAgentSession,
    type SessionRow,
    type SafeKillInspection,
  } from './sessions';
  import { formatCostMicros, formatTokens, sessionUsageTokens, decideRelatedSession } from './sessions';
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
  import { accountByUuid, accountEmailTier, sessionAccountUuid, type AccountRow } from './accounts';
  import { timeAgo } from './session_status';
  import { applySessionRename, renameKeyHandler } from './session_rename';
  import PromptComposer from './PromptComposer.svelte';
  import WatchSummary from './WatchSummary.svelte';
  import ReviewDialog from './ReviewDialog.svelte';
  import { reviewerOf } from './review_scope';
  import { prEvidenceLine, reviewDecisionWords } from './prs';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import DialogSheet from './DialogSheet.svelte';
  import KillDialog from './KillDialog.svelte';
  import TasksPanel from './TasksPanel.svelte';
  import TicketCard from './TicketCard.svelte';
  import LocalWorkspaceCard from './LocalWorkspaceCard.svelte';
  import PrResult from './PrResult.svelte';
  import { assessRow, hasReading } from './evidence';
  import SessionTasks from './SessionTasks.svelte';
  import ProposedBy from './ProposedBy.svelte';
  import RenameLabelSheet from './RenameLabelSheet.svelte';
  import { proposalFor } from './proposals';
  import Timeline from './Timeline.svelte';
  import TimelineWorkProposal from './TimelineWorkProposal.svelte';
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
  import {
    archiveOneSession,
    copySessionTranscript,
    noOtherLogin,
    sendModelChange,
    sessionActionRequest,
    takeSessionAction,
    type SessionActionId,
  } from './session_actions';
  import ForkSheet from './ForkSheet.svelte';
  import RewindSheet from './RewindSheet.svelte';
  import { suggestedForkName } from './reply_actions';
  import { MODEL_OPTIONS, modelShortLabel } from './conversation';
  import { switchTarget } from './account_limits';
  import { archiveBlocked } from './kill_check';
  import Meter from './kit/Meter.svelte';
  import { accountUsage } from './account_usage_store';
  import { leftPct, loginHeadroomText } from './account_usage';
  import { goTo } from './destination';
  import { shortcutLabel } from './shortcuts';
  import { detectMac } from './terminal_keys';

  // Two layouts of one session (UX audit 2026-10-09, I1–I6 and D1–D3): the
  // inspector beside the conversation (Main board) is a quick read plus the
  // everyday actions; the Details tab (SessionDetails board) is everything.
  let { session, variant = 'tab' }: { session: SessionRow; variant?: 'tab' | 'inspector' } = $props();

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

  // The session's own Fork…, Rewind…, Switch account…, Change model…,
  // Copy transcript and Archive (gap plan G1.11): each the backend command
  // its per-turn or bulk twin already runs, gated the same way.
  /** Fork and Rewind are `rewind_conversation`: `own`, a copy of the
   *  transcript (`share.ts`). */
  const rewindBlocked = $derived(
    hubActionBlocked('rewind_conversation', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'rewind_conversation'),
  );
  /** A read: a watcher may copy what they may read. */
  const copyTranscriptBlocked = $derived($sessionBlocked(session, 'session_conversation'));
  const archiveActionBlocked = $derived(
    archiveBlocked([session]) ?? $sessionBlocked(session, 'tidy_apply'),
  );
  const hasPaneConversation = $derived(
    !hasNoPane(session) && session.kind !== 'shell' && session.claude_session_id != null,
  );
  const canArchive = $derived(session.work != null && session.work.archived_at == null);
  let forkOpen = $state(false);
  let rewindOpen = $state(false);
  let modelOpen = $state(false);
  let modelPick = $state('');
  let copyingTranscript = $state(false);
  let archiving = $state(false);
  // Each opener is reached only through a gate (the button's `disabled`,
  // `rowActions`' `blocked()`), and each sheet re-reads the gate itself.
  function openFork() {
    forkOpen = true;
  }
  function openRewind() {
    rewindOpen = true;
  }
  function openModel() {
    if (sendPromptBlocked !== null) return;
    modelPick = '';
    modelOpen = true;
  }
  function onChangeModel() {
    modelOpen = false;
    if (sendPromptBlocked !== null || !modelPick) return;
    if (sendModelChange(session, modelPick)) push({ kind: 'info', message: `Sent /model ${modelPick}` });
  }
  async function onCopyTranscript() {
    if (copyingTranscript || copyTranscriptBlocked !== null) return;
    copyingTranscript = true;
    await copySessionTranscript(session);
    copyingTranscript = false;
  }
  async function onArchive() {
    if (archiving || archiveActionBlocked !== null) return;
    archiving = true;
    await archiveOneSession(session);
    archiving = false;
  }

  // Look up the parent project (if any) for context.
  const parentProject = $derived(
    session.project_id === null
      ? null
      : ($projectById.get(session.project_id) ?? null),
  );

  const hostRow = $derived($hostByAlias.get(session.host_alias) ?? null);
  const accountUuid = $derived(sessionAccountUuid(session, hostRow?.account_uuid));
  const accountRow = $derived(accountUuid ? ($accountByUuid.get(accountUuid) ?? null) : null);

  const worktree = $derived(parentProject?.worktrees.find((w) => w.id === session.worktree_id) ?? null);
  const usageSnap = $derived(accountUuid ? ($accountUsage[accountUuid] ?? null) : null);
  const fiveHour = $derived(usageSnap?.usage?.five_hour ?? null);
  const week = $derived(usageSnap?.usage?.seven_day ?? null);
  const prNumber = $derived(session.pr_url?.match(/\/pull\/(\d+)/)?.[1] ?? null);
  const ORIGIN_WORDS: Record<string, string> = {
    person: 'You',
    operator: 'Control',
    mission: 'A mission',
    background: 'Background agent',
    token: 'Control API',
    routine: 'A routine',
  };
  const startedBy = $derived(
    (session.origin ? ORIGIN_WORDS[session.origin] : undefined) ??
      (session.started_at ? 'Fleet' : 'Outside fleet'),
  );
  const inspectorChord = shortcutLabel('inspector', detectMac(typeof navigator === 'undefined' ? undefined : navigator));

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
  /** Each login with its account's headroom (gap plan G2.7, the
   *  FormsSession board's Switch login): "5h 75% · week 85% left",
   *  "weekly limit until Fri 11:00". '' is the host's own login. */
  const loginOptions = $derived.by(() => {
    const current = session.claude_profile ?? '';
    const hostUuid = hostRow?.account_uuid ?? null;
    const rows = [
      { value: '', name: 'Host login', email: hostUuid ? ($accountByUuid.get(hostUuid)?.email ?? null) : null, uuid: hostUuid, loggedIn: true },
      ...loginChoices.map((p) => ({ value: p.name, name: p.name, email: p.email, uuid: p.account_uuid, loggedIn: p.account_uuid != null || p.email != null })),
    ];
    return rows.map((o) => ({
      ...o,
      current: o.value === current,
      headroom: o.uuid ? loginHeadroomText($accountUsage[o.uuid] ?? null, nowSec) : null,
    }));
  });
  function loginOptionText(o: (typeof loginOptions)[number]): string {
    const who = o.email ? ` (${o.email})` : o.loggedIn ? '' : ' (not logged in)';
    return `${o.name}${who}${o.headroom ? ` · ${o.headroom}` : ''}${o.current ? ' · current' : ''}`;
  }
  /** The login the headroom rule proposed (`switchTarget`), shown as a
   *  proposal while the pick still holds it. A rule, never Jev: account
   *  limits choose by numbers (`ai_proposal.ts` NEVER_DECIDES). */
  let loginProposal = $state<string | null>(null);
  const loginTarget = $derived(loginPick ?? session.claude_profile ?? '');
  let confirmingSwitch = $state(false);
  /** Switch account…: the same switch as the Login row, from a dialog that
   *  proposes the login on this host with the most headroom. */
  let switchOpen = $state(false);
  const switchAccountBlocked = $derived(restartBlocked ?? noOtherLogin(session, hostRow));
  async function openSwitchAccount() {
    if (switchAccountBlocked !== null) return;
    loginPick = null;
    loginProposal = null;
    switchOpen = true;
    const id = session.id;
    const t = await switchTarget(session);
    // Only a proposal: a pick the person already made, or another session
    // selected meanwhile, keeps what it has.
    if (t && switchOpen && loginPick === null && session.id === id) {
      loginPick = t.profile ?? '';
      loginProposal = loginPick;
    }
  }
  // The pane is not keyed by session: a login picked (or a switch being
  // confirmed) on one session must not carry over to the next one selected,
  // where Switch would restart it under that pick (review r07).
  let pickFor = untrack(() => session.id);
  $effect.pre(() => {
    const id = session.id;
    untrack(() => {
      if (id === pickFor) return;
      pickFor = id;
      loginPick = null;
      confirmingSwitch = false;
      switchOpen = false;
      forkOpen = false;
      rewindOpen = false;
      modelOpen = false;
    });
  });
  async function onSwitchLogin() {
    confirmingSwitch = false;
    switchOpen = false;
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
  // the same thing, when it is not listed above already. Nothing is
  // stopped or merged.
  const relatedProposal = $derived(proposalFor(session, 'related_session'));
  const relatedPartner = $derived.by(() => {
    const m = /^s(\d+)$/.exec(relatedProposal?.value ?? '');
    if (!m) return null;
    const id = Number(m[1]);
    if (related.some((r) => r.id === id)) return null;
    return $sessions.find((s) => s.id === id && s.id !== session.id) ?? null;
  });
  // Link / Not related (M15 G4.3): a linked partner is listed like a
  // sibling; a proposed one carries the two answers until someone decides.
  const relatedLinked = $derived(relatedProposal?.linked === true);
  const proposedRelated = $derived(relatedLinked ? null : relatedPartner);
  const linkedRelated = $derived(relatedLinked ? relatedPartner : null);
  const relatedDecideBlocked = $derived(
    hubActionBlocked('decide_related_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(session, 'decide_related_session'),
  );
  let relatedDeciding = $state(false);
  async function decideRelated(linked: boolean): Promise<void> {
    const runId = relatedProposal?.run_id;
    if (runId == null || relatedDeciding || $sessionBlocked(session, 'decide_related_session') !== null) return;
    relatedDeciding = true;
    const r = await decideRelatedSession(session.id, runId, linked);
    relatedDeciding = false;
    if (!r.ok) pushError(r.error, linked ? 'Linking the session failed' : 'Saving “Not related” failed');
  }

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
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = false), 1500);
  }
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  onDestroy(() => clearTimeout(copiedTimer));

  let renameInput: HTMLInputElement | undefined = $state();

  async function beginEdit(mode: 'label' | 'tmux') {
    renaming = mode;
    renameValue = mode === 'label' ? (session.friendly_name ?? '') : session.tmux_name;
    await tick();
    renameInput?.focus();
    renameInput?.select();
  }

  const beginRename = () => beginEdit('tmux');
  // "Rename and label…" (gap plan G2.7): the name and the label (the
  // session's tags) in one sheet. The title's inline editor stays for tmux.
  let renameLabelOpen = $state(false);
  function openRenameLabel() {
    if (setFriendlyNameBlocked !== null) return;
    renameLabelOpen = true;
  }

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
  /** GitHub's review decision on this session's PR, in words. */
  // G7.9: "15/15 checks · no reviews" beside the PR in the inspector.
  const prLine = $derived(session.pr_url ? prEvidenceLine(session.pr_evidence) : '');
  const prReview = $derived(session.pr_url ? reviewDecisionWords(session.pr_evidence?.review_decision) : null);
  const reviewsShown = $derived(session.kind !== 'external' || reviewsOfThis.length > 0 || prReview !== null);

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
    label: { blocked: () => setFriendlyNameBlocked, run: openRenameLabel },
    rename: { blocked: () => renameBlocked, run: beginRename },
    restart: { blocked: () => restartBlocked, run: askRestart },
    repair: { blocked: () => (repairing ? 'Repairing…' : repairBlocked), run: askRepair },
    send_prompt: { blocked: () => sendPromptBlocked, run: openComposer },
    review: { blocked: () => reviewBlocked, run: () => (reviewOpen = true) },
    fork: { blocked: () => rewindBlocked, run: openFork },
    rewind: { blocked: () => rewindBlocked, run: openRewind },
    switch_account: { blocked: () => switchAccountBlocked, run: () => void openSwitchAccount() },
    change_model: { blocked: () => sendPromptBlocked, run: openModel },
    copy_transcript: { blocked: () => copyTranscriptBlocked, run: () => void onCopyTranscript() },
    archive: { blocked: () => archiveActionBlocked, run: () => void onArchive() },
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

<!-- One component, two layouts (UX audit 2026-10-09, I1-I6 and D1-D3).
     `inspector` is the Main board's compact column beside the
     conversation: facts, a few actions, Share and Kill at the foot. `tab` is
     the SessionDetails board: Facts tiles, Timeline, Related sessions and
     every action grouped Steer / Place / Share. Both run the same handlers
     and dialogs below, so nothing moves between them but the layout. -->
{#snippet act(testid: string, label: string, run: () => void, blocked: string | null, title = '', cls = 'btn btn--quiet is-bounded')}
  <button class={cls} onclick={run} disabled={blocked !== null} title={blocked ?? title} data-testid={testid}>{label}</button>
{/snippet}

{#snippet tagChips()}
  {#each session.tags as t (t)}<span class="tag-chip">{t}</span>{/each}
{/snippet}

{#snippet renameField()}
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
  {/if}
{/snippet}

{#snippet statusChip()}
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
  {:else}
    <span class="status status-{session.status}">{session.status}</span>
  {/if}
{/snippet}

{#snippet hostValue()}
  <span class="host-dot" class:online={hostRow?.reachable} aria-hidden="true"></span>{session.host_alias}{#if hostRow}{' · '}{hostRow.reachable ? 'online' : 'offline'}{/if}
{/snippet}

{#snippet prValue()}
  {#if session.pr_url}
    <a class="pr-link" href={session.pr_url} target="_blank" rel="noreferrer">{prNumber ? `#${prNumber}` : session.pr_url.replace(/^https:\/\/github\.com\//, '')}</a>
    {#if session.ci_status}
      <span
        class="chip"
        data-testid="details-ci"
        style="color: {ciStatusColor(session.ci_status)}; border-color: color-mix(in srgb, {ciStatusColor(session.ci_status)} 33%, transparent);"
        title="CI checks: {session.ci_status}"
      >{ciStatusLabel(session.ci_status)}</span>
    {/if}
    {#if prLine}
      <span class="pr-line" data-testid="details-pr-line">{prLine}</span>
    {/if}
  {/if}
{/snippet}

{#snippet moveSteps()}
  {#if session.kind !== 'external'}
    {#if moveBackOrigin}
      {@render act('details-move-back', `⇄ Move back to ${moveBackOrigin.fromHost}`, openMoveBack, moveBlocked, 'Move this session back to the host it came from')}
    {/if}
    {#if unresolvedMove}
      {@render act('details-finish-move', `Finish the move to ${unresolvedMove.toHost}…`, openFinishOrUndo, moveBlocked, 'Open the Transfer sheet to finish this move')}
      {@render act('details-undo-move', 'Undo the move…', openFinishOrUndo, moveBlocked, 'Open the Transfer sheet to undo this move')}
    {/if}
    {#if unresolvedWaitRec}
      {@render act('details-resume-wait', `⇄ Waiting to move to ${unresolvedWaitRec.toHost}…`, openWait, moveBlocked, 'Open the Transfer sheet for this pending move')}
    {/if}
  {/if}
{/snippet}

{#snippet safeKillPill()}
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
{/snippet}

{#snippet relatedRow(r: SessionRow, testid: string, sub: string)}
  <button class="related-row" data-testid={testid} onclick={() => selectSessionExplicitly(r)}>
    <span class="rel-dot" data-status={r.status} aria-hidden="true"></span>
    <span class="rel-text">
      <span class="sess-name">{r.friendly_name ?? r.tmux_name}</span>
      <span class="rel-sub">{sessionStatusWord(r)} · {r.host_alias}{sub ? ` · ${sub}` : ''} · {formatRelative(r.last_activity_at)}</span>
    </span>
  </button>
{/snippet}

{#if variant === 'inspector'}
  <article class="details inspector-view" data-testid="session-details" data-variant="inspector">
    <header class="insp-head">
      <h2 class="insp-title">Inspector</h2>
      <span class="of-kbd" title="Show or hide the inspector">{inspectorChord}</span>
    </header>
    {@render renameField()}
    <dl class="of of-kv facts-kv" data-testid="inspector-facts">
      <dt>Host</dt>
      <dd data-testid="session-host">{@render hostValue()}</dd>
      {#if session.tags?.length}
        <dt>Label</dt>
        <dd data-testid="details-tags">{@render tagChips()}</dd>
      {/if}
      <dt>Account</dt>
      <dd data-testid="session-account">{accountRow?.email ?? accountEmailTier(accountRow)}</dd>
      {#if fiveHour || week}
        <dt>Usage</dt>
        <dd data-testid="inspector-usage">
          {#if fiveHour}5h {leftPct(fiveHour)}% left{/if}{#if fiveHour && week} · {/if}{#if week}week {leftPct(week)}%{/if}
          {#if fiveHour}<Meter value={leftPct(fiveHour) / 100} level={leftPct(fiveHour) < 10 ? 'crit' : leftPct(fiveHour) < 25 ? 'warn' : 'ok'} label="{leftPct(fiveHour)}% of the 5-hour window left" />{/if}
        </dd>
      {/if}
      {#if worktree?.branch}
        <dt>Branch</dt>
        <dd class="mono">{worktree.branch}</dd>
      {/if}
      {#if worktree}
        <dt>Worktree</dt>
        <dd class="mono">{worktree.path}</dd>
      {/if}
      {#if session.pr_url}
        <dt>Pull request</dt>
        <dd data-testid="details-pr">{@render prValue()}</dd>
      {/if}
      {#if session.work}
        <dt>Task</dt>
        <dd>{#if session.work.key}<span class="mono">{session.work.key}</span> {/if}{session.work.title}</dd>
      {/if}
      <dt>Started by</dt>
      <dd>{startedBy}, {formatRelative(sessionStart(session))}</dd>
      {#if (session.usage_cost_micros ?? 0) > 0}
        <dt>Cost</dt>
        <dd class="tnum" data-testid="details-cost">{formatCostMicros(session.usage_cost_micros)}</dd>
      {/if}
    </dl>
    {@render safeKillPill()}
    {#if session.kind !== 'external'}
      <section class="insp-actions" data-testid="details-actions">
        <h3>Actions</h3>
        <div class="btn-row">
          {#if session.kind !== 'shell'}
            {@render act('send-prompt-from-details', 'Send prompt…', openComposer, sendPromptBlocked)}
          {/if}
          {#if canMove}
            {@render act('move-from-details', 'Move to host…', openMove, moveBlocked, 'Continue this conversation on another host: same branch, same Claude session')}
          {/if}
          {@render act('open-review', 'Start a review run…', () => (reviewOpen = true), reviewBlocked)}
          {#if hasPaneConversation}
            {@render act('fork-from-details', 'Fork…', openFork, rewindBlocked, 'A new session from this conversation; this one keeps running')}
          {/if}
          {#if canSwitchLogin}
            {@render act('switch-account-from-details', 'Switch account…', openSwitchAccount, switchAccountBlocked, 'Resume this conversation under another login on this host')}
          {/if}
          {@render act('restart-from-details', 'Restart…', askRestart, restartBlocked)}
          {@render moveSteps()}
        </div>
        <p class="more-hint">
          Rename, rewind, model, recreate and more:
          <button type="button" class="hint-link" data-testid="inspector-open-details" onclick={() => goTo('details')}>Details</button>
        </p>
      </section>
    {/if}
    <WatchSummary {session} />
    <footer class="insp-foot">
      {@render act('share-from-details', 'Share…', openShare, shareBlocked, 'Share this session with one person — watch or drive, revocable, and never a terminal')}
      <span class="grow"></span>
      {#if canArchive}
        {@render act('archive-from-details', 'Archive', onArchive, archiving ? 'Archiving…' : archiveActionBlocked, 'Put this session in its work’s Done; it keeps running, with Undo')}
      {/if}
      {#if isInactiveAgent(session)}
        {@render act('remove-from-list-details', 'Remove from list', onRemoveFromList, dismissAgentBlocked, 'Hide this inactive agent until it becomes active again')}
      {:else if session.kind !== 'external'}
        {@render act('kill-from-details', 'Kill session…', askKill, killBlocked, '', 'btn btn--quiet is-bounded kill')}
      {/if}
    </footer>
  </article>
{:else}
  <article class="details tab-view" data-testid="session-details" data-variant="tab">
   <div class="tab-grid">
   <div class="tab-main">
    {@render renameField()}

    <section class="block">
      <h3>Facts</h3>
      <div class="tiles" data-testid="details-facts">
        <div class="tile">
          <span class="tile-label">Status</span>
          <span class="tile-value">{@render statusChip()}</span>
        </div>
        <div class="tile">
          <span class="tile-label">Context</span>
          <span class="tile-value">
            {#if ctxLevel !== null && session.context_pct !== null}
              <span data-testid="details-context" data-level={ctxLevel} style="color: {contextColor(ctxLevel)};" title="Context window used"
                >{Math.round(session.context_pct)}%</span
              >{#if session.context_tokens}<span class="muted"> · {formatTokens(session.context_tokens)}</span>{/if}
            {:else}<span class="muted">—</span>{/if}
          </span>
        </div>
        <div class="tile">
          <span class="tile-label">Cost</span>
          <span class="tile-value tnum">{#if (session.usage_cost_micros ?? 0) > 0}{formatCostMicros(session.usage_cost_micros)}{:else}<span class="muted">—</span>{/if}<span class="muted"> · {formatElapsed(sessionStart(session), nowSec)}</span></span>
        </div>
        <div class="tile">
          <span class="tile-label">Host</span>
          <span class="tile-value" data-testid="session-host">{@render hostValue()}</span>
        </div>
        <div class="tile">
          <span class="tile-label">Account · profile</span>
          <span class="tile-value" data-testid="session-account">{accountEmailTier(accountRow)}{#if session.claude_profile}<span class="muted"> · {session.claude_profile}</span>{/if}</span>
        </div>
        <div class="tile">
          <span class="tile-label">PR · CI</span>
          {#if session.pr_url}<span class="tile-value" data-testid="details-pr">{@render prValue()}</span>{:else}<span class="tile-value muted">—</span>{/if}
        </div>
      </div>
    </section>

    <dl class="of of-kv meta">
      {#if session.tags?.length}
        <dt>Label</dt>
        <dd data-testid="details-tags">{@render tagChips()}</dd>
      {/if}
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
            {#each loginOptions as o (o.value)}
              <option value={o.value}>{loginOptionText(o)}</option>
            {/each}
          </select>
          {#if loginTarget !== (session.claude_profile ?? '')}
            <button
              class="btn btn--quiet is-bounded"
              data-testid="session-login-switch"
              onclick={() => (confirmingSwitch = true)}
              disabled={restartBlocked !== null}
              title={restartBlocked ?? ''}
            >Switch…</button>
          {/if}
        </dd>
      {/if}
      <dt>tmux session</dt>
      <dd class="mono" data-testid="details-tmux-name">{session.tmux_name}</dd>
      <dt>Project</dt>
      <dd>
        {#if parentProject}
          {parentProject.project.owner}/{parentProject.project.repo}
        {:else}
          <span class="muted">unmapped (orphan)</span>
        {/if}
      </dd>
      {#if worktree}
        <dt>Worktree</dt>
        <dd class="mono">{worktree.path}{#if worktree.branch}<span class="muted"> · {worktree.branch}</span>{/if}</dd>
      {/if}
      <dt>Started</dt>
      <dd>{startedBy}, {formatRelative(session.created_at)}</dd>
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
      {#if session.pr_url && hasReading(assessRow(session, nowSec))}
        <dt>Result</dt>
        <dd data-testid="details-pr-result"><PrResult {session} {nowSec} /></dd>
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

    <Timeline
      sessionId={session.id}
      refreshKey={`${session.turn_seq}|${session.status}|${session.claude_status}|${session.stuck_kind}|${session.last_prompt}|${session.safe_kill_state}`}
      onEvents={(e) => (timelineEvents = e)}
    >
      {#snippet head()}<TimelineWorkProposal {session} />{/snippet}
    </Timeline>

    {#if related.length > 0 || relatedPartner}
      <section class="block related" data-testid="related-sessions">
        <h3>Related sessions <span class="count">{related.length + (relatedPartner ? 1 : 0)}</span></h3>
        <ul class="related-list">
          {#each related as r (r.id)}
            <li>{@render relatedRow(r, 'related-row', accountEmailTier(accountForRow(r)))}</li>
          {/each}
          {#if linkedRelated}
            <li data-testid="related-linked">{@render relatedRow(linkedRelated, 'related-linked-row', 'linked · same work')}</li>
          {/if}
          {#if proposedRelated}
            {@const r = proposedRelated}
            <li class="proposed" data-testid="related-proposed">
              {@render relatedRow(r, 'related-proposed-row', 'same work?')}
              <ProposedBy proposal={relatedProposal} field="related_session" testid="related-proposed-by" />
              {#if relatedProposal?.run_id != null}
                <div class="btn-row related-decide">
                  <button
                    type="button"
                    class="btn is-bounded"
                    data-testid="related-link"
                    disabled={relatedDeciding || relatedDecideBlocked !== null}
                    title={relatedDecideBlocked ?? 'Same work: keep it listed here'}
                    onclick={() => void decideRelated(true)}>Link</button
                  >
                  <button
                    type="button"
                    class="btn btn--quiet is-bounded"
                    data-testid="related-not-related"
                    disabled={relatedDeciding || relatedDecideBlocked !== null}
                    title={relatedDecideBlocked ?? 'Not the same work: stop suggesting it'}
                    onclick={() => void decideRelated(false)}>Not related</button
                  >
                </div>
              {/if}
            </li>
          {/if}
        </ul>
      </section>
    {/if}

    <!-- Reviews (SessionDetails board): the PR's review decision GitHub
         reports, each review run of this session with who ran it, and Start
         a review run. Only what is recorded: a run's verdict is its
         session's state, never a findings count nobody measured. -->
    {#if reviewsShown}
      <section class="block related reviews" data-testid="reviews-panel">
        <h3>Reviews{#if reviewsOfThis.length > 0} <span class="count">{reviewsOfThis.length}</span>{/if}</h3>
        {#if prReview}
          <p class="pr-review" data-testid="reviews-pr-decision" data-decision={session.pr_evidence?.review_decision}>
            {prNumber ? `PR #${prNumber}` : 'Pull request'} · {prReview}
          </p>
        {/if}
        {#if reviewsOfThis.length > 0}
          <ul class="related-list">
            {#each reviewsOfThis as r (r.id)}
              <li>
                {@render relatedRow(r, 'reviews-row', reviewerOf(r))}
              </li>
            {/each}
          </ul>
        {:else if !prReview}
          <p class="muted" data-testid="reviews-empty">No review run yet.</p>
        {/if}
        {#if session.kind !== 'external'}
          <div class="btn-row">
            {@render act('open-review', 'Start a review run…', () => (reviewOpen = true), reviewBlocked, 'A read-only Claude Code session reviews this one: pick a skill and what it reads')}
          </div>
        {/if}
      </section>
    {/if}

    <TasksPanel sessionId={session.id} />
    <LocalWorkspaceCard {session} />
    <WatchSummary {session} />
   </div>

   <!-- Actions sit beside the facts (SessionDetails board), below them
        when the column is narrow. -->
   <div class="tab-side">
    <section class="block actions" data-testid="details-actions">
      <h3>Actions</h3>
      {@render safeKillPill()}
      {#if session.kind !== 'external'}
        <div class="group" data-testid="actions-steer">
          <h4>Steer</h4>
          <div class="btn-row">
            {#if session.kind !== 'shell'}
              {@render act('send-prompt-from-details', 'Send prompt…', openComposer, sendPromptBlocked, '', 'btn btn--primary')}
            {/if}
            {#if hasPaneConversation}
              {@render act('fork-from-details', 'Fork…', openFork, rewindBlocked, 'A new session from this conversation; this one keeps running')}
              {@render act('rewind-from-details', 'Rewind…', openRewind, rewindBlocked, 'Take the conversation back to before a turn; files stay as they are')}
            {/if}
            {#if canSwitchLogin}
              {@render act('switch-account-from-details', 'Switch account…', openSwitchAccount, switchAccountBlocked, 'Resume this conversation under another login on this host')}
              {@render act('change-model-from-details', 'Change model…', openModel, sendPromptBlocked, 'Send /model to this session')}
            {/if}
          </div>
        </div>
        <div class="group" data-testid="actions-place">
          <h4>Place</h4>
          <div class="btn-row">
            {#if canMove}
              {@render act('move-from-details', 'Move to host…', openMove, moveBlocked, 'Continue this conversation on another host: same branch, same Claude session')}
            {/if}
            {@render moveSteps()}
            {@render act('restart-from-details', 'Restart…', askRestart, restartBlocked)}
            {@render act('recreate-from-details', 'Recreate…', askRecreate, recreateBlocked)}
            {#if !hasNoPane(session) && session.project_id !== null}
              {@render act('repair-from-details', 'Repair workspace…', askRepair, repairing ? 'Repairing…' : repairBlocked, 'Recreate a deleted worktree directory, re-register it with git, and respawn the pane in it')}
            {/if}
            {@render act('label-from-details', 'Rename and label…', openRenameLabel, setFriendlyNameBlocked)}
            {@render act('rename-from-details', 'Rename tmux session', beginRename, renameBlocked)}
          </div>
        </div>
        <div class="group" data-testid="actions-share">
          <h4>Share</h4>
          <div class="btn-row">
            {@render act('share-from-details', 'Share…', openShare, shareBlocked, 'Share this session with one person — watch or drive, revocable, and never a terminal')}
            {#if session.kind !== 'shell' && session.claude_session_id != null}
              {@render act('copy-transcript-from-details', 'Copy transcript', onCopyTranscript, copyingTranscript ? 'Copying…' : copyTranscriptBlocked, 'Copy this conversation as Markdown')}
            {/if}
            {#if !hasNoPane(session) && detailsOwned}
              <button class="btn btn--quiet is-bounded" onclick={onCopy} data-testid="copy-attach" title="Copy the command that attaches this session in another terminal">
                {copied ? '✓ Copied' : 'Copy tmux attach'}
              </button>
              <code class="cmd" data-testid="attach-command">{attachCommand}</code>
            {/if}
          </div>
        </div>
      {:else}
        <div class="btn-row">
          {@render act('label-from-details', 'Rename and label…', openRenameLabel, setFriendlyNameBlocked)}
        </div>
      {/if}
      {#if session.kind !== 'external'}
        <div class="danger-row">
          {#if canArchive}
            {@render act('archive-from-details', 'Archive', onArchive, archiving ? 'Archiving…' : archiveActionBlocked, 'Put this session in its work’s Done; it keeps running, with Undo')}
          {/if}
          {#if isInactiveAgent(session)}
            {@render act('remove-from-list-details', 'Remove from list', onRemoveFromList, dismissAgentBlocked, 'Hide this inactive agent until it becomes active again')}
          {:else}
            {#if session.kind !== 'shell' && session.status === 'running' && session.safe_kill_state !== 'requested'}
              {@render act('safe-kill-from-details', 'Clean up (commit and push first)…', askSafeKill, inspectSafeKillBlocked)}
            {/if}
            {@render act('kill-from-details', 'Kill session…', askKill, killBlocked, '', 'btn btn--quiet is-bounded kill')}
          {/if}
        </div>
      {/if}
    </section>
   </div>
   </div>
  </article>
{/if}

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

{#if renameLabelOpen}
  <RenameLabelSheet {session} onclose={() => (renameLabelOpen = false)} />
{/if}

{#if forkOpen}
  <ForkSheet sessionId={session.id} anchor={null} suggestedName={suggestedForkName(session)} onclose={() => (forkOpen = false)} />
{/if}

{#if rewindOpen}
  <RewindSheet {session} onclose={() => (rewindOpen = false)} />
{/if}

{#if switchOpen}
  <DialogSheet
    title="Switch login for this session"
    lead="The agent restarts with the other account and keeps the conversation. Anything it is doing right now is lost."
    verb="Switch and restart"
    busyVerb="Switching…"
    canConfirm={loginTarget !== (session.claude_profile ?? '') && switchAccountBlocked === null}
    confirmTitle={switchAccountBlocked ?? (loginTarget === (session.claude_profile ?? '') ? 'Pick another login.' : null)}
    onconfirm={onSwitchLogin}
    onclose={() => {
      switchOpen = false;
      loginPick = null;
      loginProposal = null;
    }}
    testid="switch-account-sheet"
    confirmTestid="confirm-account-switch"
  >
    <div class="login-options" role="radiogroup" aria-label="Claude login" data-testid="switch-account-pick">
      {#each loginOptions as o (o.value)}
        <label class="login-option" class:is-current={o.current} data-testid="switch-account-option">
          <input
            type="radio"
            name="switch-login"
            value={o.value}
            checked={loginTarget === o.value}
            onchange={() => (loginPick = o.value)}
          />
          <span class="login-name">{o.name}{#if o.email}<span class="muted"> · {o.email}</span>{:else if !o.loggedIn}<span class="muted"> · not logged in</span>{/if}</span>
          <span class="login-room" data-testid="switch-account-headroom">{o.headroom ?? ''}{o.current ? (o.headroom ? ' · current' : 'current') : ''}</span>
        </label>
      {/each}
    </div>
    <ProposedBy
      proposal={loginProposal !== null && loginTarget === loginProposal
        ? { value: loginProposal || 'host', source: 'rule', reason: 'most left on this host' }
        : null}
      field="login"
      stated
      onchange={() => (loginPick = session.claude_profile ?? '')}
      testid="switch-account-proposed"
    />
  </DialogSheet>
{/if}

{#if modelOpen}
  <ConfirmDialog
    title="Change model?"
    confirmLabel="Change"
    confirmDisabled={modelPick === '' || sendPromptBlocked !== null}
    onconfirm={onChangeModel}
    oncancel={() => (modelOpen = false)}
    confirmTestId="confirm-model-change"
  >
    <label class="dialog-field">
      <span>Model{#if session.model} · now {modelShortLabel(session.model)}{/if}</span>
      <select aria-label="Model" data-testid="change-model-pick" bind:value={modelPick}>
        <option value="" disabled>Pick a model</option>
        {#each MODEL_OPTIONS as o (o.value)}
          <option value={o.value}>{o.label}</option>
        {/each}
      </select>
    </label>
    Sends <code>/model</code> to the session, as the composer's model picker does. It
    applies from the next turn.
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
  /* Two layouts (UX audit 2026-10-09): the inspector's quick read and the
     Details tab's everything. */
  .inspector-view { gap: var(--space-3); padding: var(--space-3) var(--space-4); }
  .tab-view { padding: var(--space-4) var(--space-6); container-type: inline-size; }
  .tab-grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--space-6); max-width: 1280px; }
  .tab-main,
  .tab-side { display: flex; flex-direction: column; gap: var(--space-4); min-width: 0; }
  .tab-side .actions { flex-direction: column; flex-wrap: nowrap; align-items: stretch; gap: var(--space-3); }
  .tab-side .cmd { display: block; width: 100%; box-sizing: border-box; overflow-wrap: anywhere; white-space: normal; }
  @container (min-width: 820px) {
    .tab-grid { grid-template-columns: minmax(0, 1fr) minmax(260px, 360px); }
    .tab-side { position: sticky; top: 0; align-self: start; }
  }
  .insp-head { display: flex; align-items: center; gap: var(--space-2); }
  .insp-title { margin: 0; font-size: var(--text-md); font-weight: 600; flex: 1 1 auto; }
  .insp-actions h3,
  .group h4 {
    margin: 0 0 var(--space-2);
    font-size: var(--text-2xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
  }
  .insp-foot,
  .danger-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  .btn-row { display: flex; flex-wrap: wrap; gap: var(--space-2); align-items: center; }
  .grow { flex: 1 1 auto; }
  .more-hint { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--fg-muted); }
  .dialog-field { display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3); font-size: var(--text-sm); }
  .dialog-field span { color: var(--fg-muted); }
  .hint-link {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
  }
  .hint-link:hover { text-decoration: underline; }
  /* The footer sits at the foot of the column (Main board). */
  .inspector-view { min-height: 100%; box-sizing: border-box; }
  .inspector-view .insp-foot { margin-top: auto; }
  .kill { color: var(--danger); }
  .kill:hover:not(:disabled) { background: color-mix(in srgb, var(--danger) 10%, transparent); }
  .mono { font-family: var(--font-mono); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .tiles { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-2); }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .tile-label { font-size: var(--text-2xs); color: var(--fg-muted); }
  .tile-value { font-size: var(--text-sm); font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .group { display: flex; flex-direction: column; }
  .group + .group { margin-top: var(--space-3); }
  .host-dot { display: inline-block; width: 6px; height: 6px; border-radius: 50%; background: var(--fg-muted); margin-right: 4px; vertical-align: middle; }
  .host-dot.online { background: var(--status-done); }
  .rel-dot[data-status='running'] { background: var(--status-working); }
  .rel-dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; background: var(--fg-muted); }
  .rel-text { display: flex; flex-direction: column; min-width: 0; }
  .rel-sub { font-size: var(--text-2xs); color: var(--fg-muted); }
  .count { font-size: var(--text-2xs); color: var(--fg-muted); font-weight: 500; }

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
  .title-input:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }
  .sub { display: flex; gap: 0.5rem; align-items: center; font-size: var(--text-2xs); flex-wrap: wrap; }
  .login-options {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .login-option {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .login-option:hover {
    background: var(--bg-hover, var(--bg-sunk));
  }
  .login-room {
    color: var(--fg-muted);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  .tag-chip {
    display: inline-block;
    margin-right: 4px;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--bg-sunk);
    border: 1px solid var(--border);
    font-size: var(--text-2xs);
    line-height: 16px;
  }
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
  .pr-line { color: var(--fg-muted); font-size: var(--text-xs); margin-left: 6px; }
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

  .last-prompt {
    display: block;
  }
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

  .reviews .pr-review,
  .reviews .muted {
    margin: 0 0 0.4rem 0;
    font-size: var(--text-xs);
  }
  .reviews .btn-row { margin-top: 0.4rem; }
  .related-decide { display: flex; gap: 0.4rem; margin-top: 0.3rem; }
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
