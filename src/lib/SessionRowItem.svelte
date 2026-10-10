<script lang="ts">
  import { viewKey } from './shortcuts';
  import Icon from './kit/Icon.svelte';
  import { tick, type Snippet } from 'svelte';
  import {
    recreateSession,
    dismissGhostSession,
    dismissAgentSession,
    isInactiveAgent,
    showFriendlyNames,
    showRowDetails,
    lostReasonLabel,
    sessions,
    type SessionRow,
  } from './sessions';
  import { get } from 'svelte/store';
  import { selectedSession } from './selection';
  import { hostByAlias } from './hosts';
  import { hintAnchor } from './hints';
  import { bucketState, rank } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { attentionFacts, blockedLine } from './attention_facts';
  import { slideIn, wash } from './motion_catalog';
  import { accountByUuid, accountLabel } from './accounts';
  import { pushError } from './toasts';
  import { rowPrompt, shortAge, timeAgo } from './session_status';
  import { hubStatus, hubBlock, hubActionBlocked } from './hub';
  import { hubConnection, isLost } from './hub_connection';
  import { sessionBlocked } from './share';
  import AnswerPrompt from './AnswerPrompt.svelte';
  import NameWorkDialog from './NameWorkDialog.svelte';
  import { pendingInputFor } from './pending_input';
  import type { WorkKey } from './work_keys';
  import WorkChip from './WorkChip.svelte';
  import AccountPill from './AccountPill.svelte';
  import {
    branchMates,
    confirmSessionWork,
    describeEvidence,
    linkSessionWork,
    rejectSessionWork,
    rejectWorkLink,
    sessionWorkLinks,
    setWorkProjectTrust,
    unlinkSessionWork,
    workWhy,
    JEV_RULE,
    crossOrgOf,
    crossOrgSentence,
    type WorkLink,
  } from './work';
  import { fleetSettings, SETTING_KEYS } from './fleet_settings';
  import type { Result } from './result';
  import { orgs } from './orgs';
  import SessionStatusChip from './SessionStatusChip.svelte';
  import SessionRowDetails from './SessionRowDetails.svelte';
  import SessionRowMeta from './SessionRowMeta.svelte';
  import LimitActions from './LimitActions.svelte';
  import { uiDensity } from './prefs';
  import { localWorkspaces, linkFor, badgeFor } from './local_workspaces';
  import { projectById } from './projects';
  import SessionRowMenu from './SessionRowMenu.svelte';
  import PulseSteps from './PulseSteps.svelte';
  import { sessionPulse } from './session_loaders';
  import { startingSessions } from './session_starting';

  // Rename and selection state stay in the Sidebar (they must survive a
  // sessions store refresh); the row gets them as props and calls back.
  let {
    sess,
    selectMode,
    isChecked,
    isRenaming,
    renameMode = null,
    renameValue = $bindable(),
    renameInput = $bindable(),
    renameError,
    relatedCount,
    nowSec,
    readOnly = false,
    workKey = null,
    workOf = undefined,
    onSelectSession,
    onKeySession,
    toggleSelected,
    beginRename,
    beginLabelEdit,
    onRenameKey,
    commitRename,
    askRecreate,
    askRestart,
    askKill,
    orgColor = null,
    trailing = undefined,
  }: {
    sess: SessionRow;
    selectMode: boolean;
    isChecked: boolean;
    isRenaming: boolean;
    /** What the inline editor changes: the display label or the tmux name. */
    renameMode?: 'label' | 'tmux' | null;
    renameValue: string;
    renameInput: HTMLInputElement | undefined;
    renameError: string | null;
    relatedCount: number;
    nowSec: number;
    /** True for a read-only row (the "Outside fleet" group): name + status
     *  chip only, no rename / restart / recreate / kill actions. Selecting
     *  still works. */
    readOnly?: boolean;
    /** The row's work key (work_keys.ts), drawn as a chip after the name.
     *  Null when it has none, or when the row already sits under its work
     *  group's header, which names the key. */
    workKey?: WorkKey | null;
    /** The row's work key whether or not the chip shows it (inside a work
     *  group the header names it): what the work menu's "Not this" / "Clear"
     *  act on. Defaults to `workKey`. */
    workOf?: WorkKey | null;
    onSelectSession: (sess: SessionRow, e?: MouseEvent) => void;
    /** Handles Enter/Space on the ROW. It must ignore events that bubbled
     *  up from a nested control (the action cluster, the select box, the
     *  rename input): activating a `<button>` is the default action of its
     *  own keydown, so calling `preventDefault()` here would cancel it.
     *  Sidebar's implementation guards on `e.target === e.currentTarget`. */
    onKeySession: (e: KeyboardEvent, sess: SessionRow) => void;
    toggleSelected: (sess: SessionRow) => void;
    beginRename: (sess: SessionRow, e?: Event) => unknown;
    beginLabelEdit: (sess: SessionRow, e?: Event) => unknown;
    onRenameKey: (e: KeyboardEvent) => void;
    commitRename: () => unknown;
    askRecreate: (sess: SessionRow, e?: Event) => void;
    askRestart: (sess: SessionRow, e?: Event) => void;
    askKill: (sess: SessionRow, e?: Event) => void;
    /** Work graph M5: the org's colour, drawn as a thin bar at the row's
     *  left edge — only when two or more orgs exist (the caller decides). */
    orgColor?: string | null;
    /** Drawn last inside the row: a control that belongs to it (the
     *  archived chip), so a tree row owns its own controls. */
    trailing?: Snippet;
  } = $props();

  const sessSelected = $derived($selectedSession?.id === sess.id);
  // Step 5.14: a session this window just started, until its agent is up
  // (`session_starting.ts`).
  const starting = $derived($startingSessions.has(sess.id));
  const startPulse = $derived(sessionPulse(sess));
  /** "Worktree ✓ · tmux ✓ · Claude Code starting", as the board words it. */
  const startText = $derived(
    startPulse.steps
      .map((s) => (s.state === 'done' ? `${s.label} ✓` : s.state === 'active' ? `${s.label} starting` : s.label))
      .join(' · '),
  );
  // The row's triage bucket (P13). Published as data-bucket because component
  // CSS never reaches jsdom, so this is how tests assert a row's triage state.
  const triage = $derived(
    rank(sess, { idleSecs: $attentionIdleMinutes * 60, now: nowSec, facts: $attentionFacts }),
  );
  // Redesign step 3.6: Compact is the two-line row (sans title, one meta
  // line, chips on hover); Comfortable is 0.5.4's row unchanged.
  const compact = $derived($uiDensity === 'compact');
  // A Blocked row's reason (step 2.4), in either density.
  const blockedReason = $derived(
    blockedLine(triage.bucket, sess, $attentionFacts, (u) => accountLabel($accountByUuid.get(u))),
  );
  const promptText = $derived(rowPrompt(sess));
  // The dialog this row is blocked on, straight from the row: the sidebar
  // does not probe (that would be one `capture-pane` per visible row, every
  // couple of seconds). Which is why the card re-reads the pane itself
  // before it sends anything — see AnswerPrompt.
  const answerView = $derived(
    pendingInputFor({
      rowStatus: sess.claude_status,
      rowStuck: sess.stuck_kind,
      rowPending: sess.pending_input,
      probe: null,
    }),
  );
  const primaryIsFriendly = $derived($showFriendlyNames && !!sess.friendly_name);
  const primaryName = $derived(primaryIsFriendly ? sess.friendly_name! : sess.tmux_name);
  // Line 2 names what line 1 does not: the tmux name under a friendly name,
  // else the worktree when it is not already part of the tmux name.
  const secondaryName = $derived.by((): string | null => {
    if (primaryIsFriendly) return sess.tmux_name;
    if (sess.worktree_key && !sess.tmux_name.endsWith(`--${sess.worktree_key}`)) return sess.worktree_key;
    return null;
  });

  async function doRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    // Re-asked at the call (multi-user M1, F2b): the button's `disabled` is
    // reactive, but the keyboard and a stale render are not it.
    if ($sessionBlocked(sess, 'recreate_session') !== null) return;
    const r = await recreateSession(sess.id);
    if (!r.ok) pushError(r.error, 'Recreate failed');
  }

  async function doDismissGhost(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    // Re-asked at the call (multi-user M1, F2b): the button's `disabled` is
    // reactive, but the keyboard and a stale render are not it.
    if ($sessionBlocked(sess, 'dismiss_ghost_session') !== null) return;
    const r = await dismissGhostSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Dismiss failed');
    }
  }

  /** Remove an inactive bg agent from the list. The row itself disappears
   *  via the `session:removed` event the backend emits on success. */
  async function doDismissAgent(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    // Re-asked at the call (multi-user M1, F2b): the button's `disabled` is
    // reactive, but the keyboard and a stale render are not it.
    if ($sessionBlocked(sess, 'dismiss_agent_session') !== null) return;
    const r = await dismissAgentSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Remove failed');
    }
  }

  // Review r13 (States board): with the hub lost, or the row's host known
  // unreachable, the status shown is the last one read. The row is dimmed
  // and says so, rather than reading as live.
  const stale = $derived(isLost($hubConnection) || $hostByAlias.get(sess.host_alias)?.reachable === false);

  function hostIsReachable(alias: string): boolean {
    return $hostByAlias.get(alias)?.reachable ?? false;
  }

  // kill_session (routed) does the same thing to an inactive agent that
  // dismiss_agent_session does, but this row's Kill button is hidden for an
  // inactive one (`!isInactiveAgent(sess)` below) — so a paired client sees
  // the reason instead of a control that fails at the click.
  const dismissAgentBlocked = $derived(
    hubBlock('dismiss_agent_session', $hubStatus) ?? $sessionBlocked(sess, 'dismiss_agent_session'),
  );
  // These route, so they only need the live connection to be up — plus, since
  // multi-user M1, the client's own access to THIS row. The two predicates are
  // composed rather than merged: `hubActionBlocked` takes an action name and no
  // session (so a per-session refusal cannot ride it unnoticed) and
  // `sessionBlocked` takes the row; the hub's half wins when both answer,
  // because "the hub never accepts this from a client" is true of every session
  // and so the more useful sentence. `$sessionBlocked` is read through its
  // store on purpose: a revoke or a narrow changes no column on the row, so a
  // version that read `sess` alone would leave these buttons enabled until the
  // next re-list.
  const killBlocked = $derived(
    hubActionBlocked('kill_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'kill_session'),
  );
  const restartBlocked = $derived(
    hubActionBlocked('restart_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'restart_session'),
  );
  const recreateBlocked = $derived(
    hubActionBlocked('recreate_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'recreate_session'),
  );
  const labelBlocked = $derived(
    hubActionBlocked('set_friendly_name', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'set_friendly_name'),
  );
  const tmuxRenameBlocked = $derived(
    hubActionBlocked('rename_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'rename_session'),
  );
  const ghostDismissBlocked = $derived(
    hubActionBlocked('dismiss_ghost_session', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'dismiss_ghost_session'),
  );
  const workBlocked = $derived(
    hubActionBlocked('link_session_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'link_session_work'),
  );
  const nameBlocked = $derived(
    hubActionBlocked('name_session_work', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'name_session_work'),
  );
  const renameWorkBlocked = $derived(
    hubActionBlocked('rename_work_item', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'rename_work_item'),
  );
  /** "Trust this project's branch names" writes fleet settings from inside the
   *  work popover: `set_work_project_trust`, `drive`. It is the one control in
   *  the popover that does not go through `workAction`. */
  const trustBlocked = $derived(
    hubActionBlocked('set_work_project_trust', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'set_work_project_trust'),
  );
  /** Whether this client may type into the row's pane at all — the gate on the
   *  inline answer card below, which is a pane write (`send_prompt`), not a
   *  status display. A watcher still sees the `blocked` chip; what they do not
   *  get is the buttons that answer for the owner. */
  const promptBlocked = $derived(
    hubActionBlocked('send_prompt', $hubStatus, $hubConnection) ??
      $sessionBlocked(sess, 'send_prompt'),
  );
  // UX audit L2: a Compact row says what the question is ("Waiting for you:
  // Allow Bash(…)?") and leaves the answer card to the conversation, the
  // Inbox's open row and ⌘K Approve; Comfortable keeps the card in the row.
  const waitingLine = $derived(
    answerView && promptBlocked === null && (answerView.question || answerView.detail)
      ? `Waiting for you: ${answerView.question || answerView.detail}`
      : null,
  );
  /**
   * The row's privacy badge (multi-user M1). Read straight off the row —
   * `visibility` plus whether an owner arrived with it — so it says the same
   * thing to every viewer and needs no derivation: it is a fact about the
   * session, not about who is looking at it.
   *
   * `null` renders nothing. That covers a hub older than M1 (neither field
   * arrives) and the one inconsistent shape the wire can produce: `private`
   * with no `owner_person_id`, because `strip_nulls` removes a null on the way
   * out and an unowned private row cannot be described honestly. Guessing
   * either way would be worse than saying nothing.
   */
  // Local workspace sync: the link on this row's worktree, if any.
  const localLink = $derived(
    linkFor(
      $localWorkspaces,
      sess,
      sess.project_id != null ? $projectById.get(sess.project_id)?.project : undefined,
    ),
  );
  const localBadge = $derived(badgeFor(localLink));
  const privacyBadge = $derived.by((): { text: string; title: string } | null => {
    if (sess.visibility === 'unclaimed') {
      return {
        text: 'unclaimed',
        title:
          'Fleet did not start this session and nobody has claimed it. Until someone does, all anyone sees of it is a per-host count.',
      };
    }
    if (sess.visibility === 'private' && sess.owner_person_id != null) {
      return {
        text: 'private',
        title:
          'Private to its owner: only they, and the people they have shared it with, can see this session.',
      };
    }
    return null;
  });

  // ── Work menu (roadmap M1b.2): set the row's work, "Not this", "Clear". ──
  const rowWork = $derived(workOf === undefined ? workKey : workOf);
  let workMenuOpen = $state(false);
  let workDraft = $state('');
  let workBusy = $state(false);

  function toggleWorkMenu(e: Event) {
    e.stopPropagation();
    workMenuOpen = !workMenuOpen;
    workDraft = '';
  }

  // Work graph M5: a link across orgs is refused with a reason; the menu
  // explains it and offers "Link anyway", which retries with the override.
  let crossOrg = $state<{ sentence: string; retry: () => Promise<Result<unknown>> } | null>(null);

  /**
   * Every work write this row can make goes through here, and the gate goes
   * here with it (multi-user M1, F2b). The menu's ENTRANCE was gated and
   * nothing inside it was: a `drive` grant narrowed to `watch` while the
   * popover was open still landed a link, a rejection, a confirm or a clear,
   * and `y`/`n` on the row reach these paths with no button in between.
   *
   * `workBlocked` is `link_session_work`, `drive` — the same tier as every
   * other per-session work write this runner performs (`reject_session_work`,
   * `unlink_session_work`, `confirm_session_work`), so one answer covers them:
   * `share.ts`'s refusal is a function of the TIER, not of the action name,
   * and a `watch` grantee is refused all four with the same sentence.
   */
  async function workAction(
    run: () => Promise<Result<unknown>>,
    failure: string,
    forced?: { what: string; retry: () => Promise<Result<unknown>> },
  ) {
    if (workBusy || workBlocked !== null) {
      workMenuOpen = false;
      return;
    }
    workBusy = true;
    const r = await run();
    workBusy = false;
    if (!r.ok) {
      const c = forced ? crossOrgOf(r.error) : null;
      if (c && forced) {
        const names = new Map($orgs.map((o) => [o.id, o.name]));
        crossOrg = { sentence: crossOrgSentence(forced.what, c, (id) => names.get(id)), retry: forced.retry };
        return;
      }
      pushError(r.error, failure);
      return;
    }
    crossOrg = null;
    workMenuOpen = false;
    workDraft = '';
  }

  function linkAnyway(e?: Event) {
    e?.stopPropagation();
    const c = crossOrg;
    if (!c) return;
    crossOrg = null;
    void workAction(c.retry, 'Set work failed');
  }

  function setWork(e?: Event) {
    e?.stopPropagation();
    const key = workDraft.trim();
    if (!key) return;
    void workAction(() => linkSessionWork(sess.id, { key }), 'Set work failed', {
      what: key.toUpperCase(),
      retry: () => linkSessionWork(sess.id, { key }, { forceCrossOrg: true }),
    });
  }

  function rejectWork(e: Event) {
    e.stopPropagation();
    const w = rowWork;
    if (!w) return;
    // A linked item is rejected by id (its key may be null); anything else
    // by the key the row shows.
    const ref = w.source === 'link' && sess.work?.item_id != null
      ? { item_id: sess.work.item_id }
      : { key: w.key };
    void workAction(() => rejectSessionWork(sess.id, ref), 'Not this failed');
  }

  // ── "Name this work…" (work graph M11.1): work with a title and no
  // ticket. A local item's title can be renamed from here too (the backend
  // refuses a ticket anyway). `status_category` stays tracker-only on the
  // wire on purpose (native item status task 4, fix round 2 reverted an
  // attempt to relax it): a paired phone derives "this is a local item,
  // not a ticket" from `status_category == null`, so making it non-null
  // for local work would silently break Rename on every phone. `kind`
  // (`tracker` | `local` | `ref`) is the newer, explicit signal — prefer
  // it; fall back to the `status_category == null && !url` heuristic only
  // when a hub is old enough not to send `kind` at all, since that hub
  // still follows the same tracker-only rule the fallback assumes. A
  // local item's LIVE status (including the working-session lift) is
  // `effective_status`, a different field entirely — irrelevant to this
  // local-vs-ticket check.
  let nameDialog = $state<
    | { mode: 'name'; sessions: { id: number; label: string }[]; branchMates?: { id: number; label: string }[] }
    | { mode: 'rename'; itemId: number; title: string; key?: string | null }
    | null
  >(null);
  const localItem = $derived(
    sess.work?.item_id != null &&
      (sess.work.kind
        ? sess.work.kind === 'local'
        : sess.work.status_category == null && !sess.work.url)
      ? { itemId: sess.work.item_id, title: sess.work.title, key: sess.work.key ?? null }
      : null,
  );

  function openNameWork(e: Event) {
    e.stopPropagation();
    if (nameBlocked !== null) return;
    workMenuOpen = false;
    nameDialog = {
      mode: 'name',
      sessions: [{ id: sess.id, label: primaryName }],
      branchMates: branchMates(sess, get(sessions)).map((s) => ({ id: s.id, label: s.friendly_name || s.tmux_name })),
    };
  }

  function openRenameWork(e: Event) {
    e.stopPropagation();
    if (!localItem || renameWorkBlocked !== null) return;
    workMenuOpen = false;
    nameDialog = { mode: 'rename', ...localItem };
  }

  function clearWork(e: Event) {
    e.stopPropagation();
    const linkId = sess.work?.link_id;
    if (linkId == null) return;
    void workAction(() => unlinkSessionWork(sess.id, linkId), 'Clear work failed');
  }

  // ── Detection (work graph M4.4): the suggestion chip, its evidence, and
  // the decisions. A suggestion never regroups the row; only Confirm does.
  const suggestion = $derived(sess.work_suggested ?? null);
  const suggestionKey = $derived<WorkKey | null>(
    suggestion && (suggestion.key || suggestion.title)
      ? {
          key: (suggestion.key || suggestion.title) as string,
          source: 'link',
          from: suggestion.title,
          why: workWhy({ ...suggestion, state: 'suggested' }),
        }
      : null,
  );
  let workLinks = $state<WorkLink[] | null>(null);
  let workInput = $state<HTMLInputElement | null>(null);
  let trustedOverride = $state<boolean | null>(null);
  const projectTrusted = $derived.by(() => {
    if (trustedOverride !== null) return trustedOverride;
    try {
      const ids: unknown = JSON.parse($fleetSettings[SETTING_KEYS.workTrustedBranchProjects] ?? '[]');
      return Array.isArray(ids) && sess.project_id != null && ids.includes(sess.project_id);
    } catch {
      return false;
    }
  });

  async function loadWorkLinks() {
    const r = await sessionWorkLinks(sess.id);
    workLinks = r.ok && Array.isArray(r.value) ? r.value : [];
  }

  function openWorkMenu(e?: Event) {
    e?.stopPropagation();
    workMenuOpen = true;
    workDraft = '';
    void loadWorkLinks();
  }

  /** The links the popover explains: suggestions, then the primary. */
  const explained = $derived(
    (workLinks ?? []).filter((l) => l.state === 'suggested' || (l.is_primary && l.state === 'confirmed')),
  );

  function linkLabel(l: WorkLink): string {
    return l.ref_key ?? (l.item_id != null ? `item ${l.item_id}` : 'work');
  }

  function confirmLink(linkId: number, e?: Event) {
    e?.stopPropagation();
    void workAction(() => confirmSessionWork(sess.id, linkId), 'Confirm failed', {
      what: 'That suggestion',
      retry: () => confirmSessionWork(sess.id, linkId, { forceCrossOrg: true }),
    });
  }

  function rejectLink(linkId: number, e?: Event) {
    e?.stopPropagation();
    void workAction(() => rejectWorkLink(sess.id, linkId), 'Not this failed');
  }

  function pickAnother(e: Event) {
    e.stopPropagation();
    workInput?.focus();
  }

  async function toggleTrust(e: Event) {
    e.stopPropagation();
    const pid = sess.project_id;
    if (pid == null) return;
    // `set_work_project_trust` is `drive` and does not go through
    // `workAction`, so it carries its own re-ask (multi-user M1, F2b).
    if (trustBlocked !== null) return;
    const on = (e.currentTarget as HTMLInputElement).checked;
    const r = await setWorkProjectTrust(pid, on);
    if (!r.ok) {
      pushError(r.error, 'Trust failed');
      return;
    }
    trustedOverride = r.value.includes(pid);
  }

  /** `y` / `n` decide the row's top suggestion, `l` links or picks. */
  function onRowKey(e: KeyboardEvent) {
    if (rowMenuOn && (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey))) {
      e.preventDefault();
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      rowMenu = { x: r.left + 24, y: r.bottom };
      return;
    }
    // The keys are the registry's `session-row` rows (step 0.1).
    const act = e.target === e.currentTarget && workBlocked === null ? viewKey('session-row', e) : null;
    if (act) {
      if (act === 'session-row.yes' && suggestion) {
        e.preventDefault();
        confirmLink(suggestion.link_id);
        return;
      }
      if (act === 'session-row.no' && suggestion) {
        e.preventDefault();
        rejectLink(suggestion.link_id);
        return;
      }
      if (act === 'session-row.link') {
        e.preventDefault();
        openWorkMenu();
        void tick().then(() => workInput?.focus());
        return;
      }
    }
    onKeySession(e, sess);
  }

  // The row's ⋯ menu and right-click (redesign step 3.10): every
  // Details action, run by Details (`session_actions.ts`). Ghost and outside-
  // fleet rows keep their own inline actions.
  const rowMenuOn = $derived(!readOnly && sess.status !== 'ghost');
  let rowMenu = $state<{ x: number; y: number } | null>(null);
  function openRowMenu(e: MouseEvent) {
    e.stopPropagation();
    if (rowMenu) {
      rowMenu = null;
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    rowMenu = { x: r.left, y: r.bottom + 2 };
  }
  function onRowContextMenu(e: MouseEvent) {
    if (!rowMenuOn || isRenaming) return;
    e.preventDefault();
    rowMenu = { x: e.clientX, y: e.clientY };
  }

  function onWorkKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === 'Enter') setWork(e);
    else if (e.key === 'Escape') workMenuOpen = false;
  }
</script>

<div
  class="sess-row"
  class:selected={sessSelected}
  class:renaming={isRenaming}
  class:checked={isChecked}
  class:stuck={sess.stuck_kind !== null}
  class:compact
  class:stale
  data-stale={stale ? 'true' : undefined}
  title={stale ? 'Last known state: no live updates until the connection is back' : undefined}
  data-testid="sess-row"
  data-density={$uiDensity}
  data-session-id={sess.id}
  data-org-color={orgColor ?? undefined}
  style:--org-color={orgColor ?? undefined}
  aria-current={sessSelected ? 'true' : undefined}
  data-stuck={sess.stuck_kind ?? undefined}
  data-bucket={triage.bucket}
  role="treeitem"
  aria-selected={sessSelected}
  tabindex="0"
  ondblclick={(e) => sess.status !== 'ghost' && !readOnly && beginLabelEdit(sess, e)}
  onclick={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onSelectSession(sess, e)}
  onkeydown={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onRowKey(e)}
  oncontextmenu={onRowContextMenu}
  use:hintAnchor={{ id: 'session-actions', when: !!sess.claude_session_id && sess.status !== 'ghost' }}
  use:slideIn={sess.id}
  use:wash={bucketState(triage.bucket)}
>
  {#if selectMode && !readOnly}
    <!-- The row is a treeitem (redesign step 7.2), so the box is a control
         of its own inside it, not a child a button would hide. It is a
         visible affordance for the row's toggle and stops propagation so
         the two never double-fire. -->
    <input
      type="checkbox"
      class="select-box"
      checked={isChecked}
      data-testid="select-box"
      aria-label="Select {sess.tmux_name}"
      onclick={(e) => { e.stopPropagation(); toggleSelected(sess); }}
    />
  {/if}
  {#if isRenaming}
    <input
      bind:this={renameInput}
      class="rename-input"
      data-testid={renameMode === 'label' ? 'label-input' : 'rename-input'}
      aria-label={renameMode === 'label'
        ? `Label for ${sess.tmux_name} (empty clears it)`
        : `New tmux session name for ${sess.tmux_name}`}
      placeholder={renameMode === 'label' ? sess.tmux_name : undefined}
      bind:value={renameValue}
      onkeydown={onRenameKey}
      onblur={commitRename}
    />
  {:else}
    {#if readOnly}
      <!-- "Outside fleet": a Claude session running entirely outside tmux.
           Read-only — name and status chip only, no actions. Checked before
           the ghost branch: a ghosted external row must not offer
           Recreate / Dismiss either. -->
      <span class="status-dot status-{sess.status}" title={sess.status} role="img" aria-label="Status: {sess.status}"></span>
      <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
      <SessionStatusChip {sess} brief />
    {:else if sess.status === 'ghost'}
      <span class="status-dot status-ghost" title="Failed · session lost" role="img" aria-label="Status: Failed, session lost"></span>
      <span class="host-badge" data-testid="host-badge" aria-label="host {sess.host_alias}">{sess.host_alias}</span>
      <span class="sess-name" title={sess.tmux_name}>{
        $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
      }</span>
      {#if sess.lost_at}
        <span class="lost-at" title="Lost at {new Date(sess.lost_at * 1000).toLocaleString()}">
          lost {timeAgo(sess.lost_at)}{#if lostReasonLabel(sess.lost_reason)}<span data-testid="lost-reason"> · {lostReasonLabel(sess.lost_reason)}</span>{/if}
        </span>
      {/if}
      <div class="row-actions">
        <button
          class="icon-btn small"
          data-testid="ghost-recreate"
          onclick={(e) => doRecreate(sess, e)}
          disabled={!hostIsReachable(sess.host_alias) || recreateBlocked !== null}
          title={recreateBlocked ?? (hostIsReachable(sess.host_alias) ? 'Recreate tmux session' : 'Host is offline')}
          aria-label="Recreate"
        >↺</button>
        <button
          class="icon-btn small danger"
          data-testid="ghost-dismiss"
          onclick={(e) => doDismissGhost(sess, e)}
          disabled={ghostDismissBlocked !== null}
          title={ghostDismissBlocked ?? 'Dismiss lost session'}
          aria-label="Dismiss"
        >×</button>
      </div>
    {:else}
      <div class="sess-lines">
        <div class="sess-line1">
          <span class="status-dot status-{sess.status}" title={sess.status} role="img" aria-label="Status: {sess.status}"></span>
          {#if relatedCount > 0}
            <span
              class="related-badge"
              data-testid="related-badge"
              role="img"
              title="{relatedCount} related session(s)"
              aria-label="{relatedCount} related sessions"
            ><Icon name="link" size={12} />{relatedCount}</span>
          {/if}
          {#if sess.kind === 'review'}
            <span class="review-badge" role="img" title="review session" aria-label="review session"><Icon name="search" size={12} /></span>
          {/if}
          {#if sess.kind === 'shell'}
            <span class="shell-badge" role="img" title="shell session" aria-label="shell session"><Icon name="terminal" size={12} /></span>
          {/if}
          {#if sess.kind === 'bg'}
            <span class="bg-badge" role="img" title="background agent" aria-label="background agent"><Icon name="agent" size={12} /></span>
          {/if}
          <span class="sess-name" title={sess.tmux_name}>{primaryName}</span>
          <!-- The chip strip. A Compact row hides it until hover or focus
               (its state is on the meta line); Comfortable lays the chips
               out as if the wrapper were not there. The work suggestion
               stays outside it: a proposal is never hidden. -->
          <span class="chips" data-testid="row-chips">
            {#if privacyBadge}
              <!-- Beside WorkChip in the line-1 chip strip: the same place every
                   other fact about the row is drawn. -->
              <span class="privacy-chip" data-testid="privacy-chip" title={privacyBadge.title}
                >{privacyBadge.text}</span
              >
            {/if}
            {#if localLink}
              <!-- Local workspace sync: shown only on a linked worktree. -->
              <span
                class="lw-dot tone-{localBadge.tone}"
                data-testid="local-sync-dot"
                title="Local workspace: {localBadge.label} — {localLink.local_path}"
                aria-label="Local workspace: {localBadge.label}"
              ></span>
              {#if (localLink.local_activity ?? 0) > 0}
                <span class="lw-changes" data-testid="local-sync-changes">changes</span>
              {/if}
            {/if}
            {#if workKey}
              <WorkChip {workKey} />
            {/if}
            {#each sess.tags ?? [] as t (t)}
              <!-- The session's label (its tags, gap plan G2.7). -->
              <span class="label-chip" data-testid="row-label" title="Label: {t}">{t}</span>
            {/each}
          </span>
          {#if suggestionKey && suggestion}
            <WorkChip
              workKey={suggestionKey}
              suggested
              proposed={suggestion.rule === JEV_RULE}
              testid="work-suggestion"
              onclick={(e) => (workBlocked === null ? openWorkMenu(e) : e.stopPropagation())}
            />
          {/if}
          <span class="chips" data-testid="row-chips">
            <SessionStatusChip {sess} />
          </span>
          {#if sess.account_uuid}
            <!-- Redesign 4.3: the account this session runs on, on every
                 Comfortable row; a Compact row shows it with the chips, on
                 hover (UX audit L1; the inspector names it too). -->
            <span class="acct"><AccountPill uuid={sess.account_uuid} /></span>
          {/if}
          {#if compact}
            <span class="sess-age" data-testid="sess-age" title={new Date(sess.last_activity_at * 1000).toLocaleString()}
              >{shortAge(sess.last_activity_at, nowSec)}</span
            >
          {/if}
          <div class="row-actions">
            {#if isInactiveAgent(sess)}
              <button
                class="icon-btn small danger"
                data-testid="remove-from-list"
                disabled={dismissAgentBlocked !== null}
                onclick={(e) => doDismissAgent(sess, e)}
                title={dismissAgentBlocked ?? 'Remove from list'}
                aria-label="Remove from list"
              >×</button>
            {/if}
            <button
              class="icon-btn small"
              data-testid="restart-session"
              onclick={(e) => askRestart(sess, e)}
              disabled={restartBlocked !== null}
              title={restartBlocked ?? 'Restart claude in this session'}
              aria-label="Restart"
            >↻</button>
            <button
              class="icon-btn small"
              data-testid="work-menu"
              onclick={(e) => (workMenuOpen ? toggleWorkMenu(e) : openWorkMenu(e))}
              disabled={workBlocked !== null}
              title={workBlocked ?? 'Work: set, "Not this", clear'}
              aria-label="Work"
              aria-expanded={workMenuOpen}
            >#</button>
            <button
              class="icon-btn small"
              data-testid="edit-label"
              onclick={(e) => beginLabelEdit(sess, e)}
              disabled={labelBlocked !== null}
              title={labelBlocked ?? 'Rename (double-click the row)'}
              aria-label="Rename"
            ><Icon name="tag" size={12} /></button>
            <button
              class="icon-btn small"
              data-testid="rename-tmux"
              onclick={(e) => beginRename(sess, e)}
              disabled={tmuxRenameBlocked !== null}
              title={tmuxRenameBlocked ?? 'Rename tmux session'}
              aria-label="Rename tmux session"
            ><Icon name="edit" size={12} /></button>
            <button
              class="icon-btn small"
              data-testid="recreate-live"
              onclick={(e) => askRecreate(sess, e)}
              disabled={!hostIsReachable(sess.host_alias) || recreateBlocked !== null}
              title={recreateBlocked ?? (hostIsReachable(sess.host_alias)
                ? 'Recreate: kill the tmux session and start it fresh in the same worktree'
                : 'Host is offline')}
              aria-label="Recreate"
            ><Icon name="recreate" size={12} /></button>
            {#if !isInactiveAgent(sess)}
              <!-- An inactive agent's daemon is gone: Remove from list is its
                   only removal action. -->
              <button
                class="icon-btn small danger"
                onclick={(e) => askKill(sess, e)}
                disabled={killBlocked !== null}
                title={killBlocked ?? 'Kill session'}
                aria-label="Kill"
              >×</button>
            {/if}
            {#if rowMenuOn}
              <button
                class="icon-btn small"
                data-testid="row-menu-open"
                onclick={openRowMenu}
                title="Every action on this session (right-click the row)"
                aria-label="More actions"
                aria-haspopup="menu"
                aria-expanded={rowMenu !== null}
              >⋯</button>
            {/if}
          </div>
        </div>
        {#if workMenuOpen}
          <!-- Every control stops its click: the panel sits inside the row,
               whose own click selects the session. -->
          <div
            class="work-menu"
            data-testid="work-menu-panel"
            role="group"
            aria-label="Work for {primaryName}"
          >
            {#if explained.length > 0}
              <div class="work-why" data-testid="work-why">
                {#each explained as l (l.id)}
                  <div class="why-link" data-testid="why-link" data-state={l.state}>
                    <span class="why-key">{linkLabel(l)}</span>
                    <span class="why-what">{workWhy(l)}</span>
                    {#each l.evidence ?? [] as ev, i (i)}
                      <span class="why-ev" data-testid="why-evidence" title={ev.snippet ?? ''}>{describeEvidence(ev)}</span>
                    {/each}
                    {#if l.state === 'suggested'}
                      <span class="why-actions">
                        <button class="work-btn" data-testid="why-confirm" disabled={workBusy || workBlocked !== null}
                          title={workBlocked ?? 'Confirm (↵ / y)'} onclick={(e) => confirmLink(l.id, e)}>Confirm</button>
                        <button class="work-btn" data-testid="why-reject" disabled={workBusy || workBlocked !== null}
                          title={workBlocked ?? 'Not this (⌫ / n): never suggested again'} onclick={(e) => rejectLink(l.id, e)}>Not this</button>
                        <button class="work-btn" data-testid="why-pick" disabled={workBusy}
                          title="Type or paste another key or ticket URL" onclick={pickAnother}>Pick another…</button>
                      </span>
                    {/if}
                  </div>
                {/each}
                {#if sess.project_id != null}
                  <!-- The label only stops the row's own click (it would
                       select the session); the checkbox is the control. -->
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                  <label class="why-trust" onclick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      data-testid="why-trust"
                      checked={projectTrusted}
                      disabled={trustBlocked !== null}
                      title={trustBlocked ?? ''}
                      onchange={toggleTrust}
                    />
                    Trust branch keys in this repo
                  </label>
                {/if}
              </div>
            {/if}
            {#if crossOrg}
              <div class="cross-org" data-testid="cross-org" role="alert">
                <span>{crossOrg.sentence}</span>
                <button
                  class="work-btn"
                  data-testid="cross-org-force"
                  disabled={workBusy || workBlocked !== null}
                  title={workBlocked ?? ''}
                  onclick={linkAnyway}>Link anyway</button
                >
                <button
                  class="work-btn"
                  data-testid="cross-org-cancel"
                  onclick={(e) => {
                    e.stopPropagation();
                    crossOrg = null;
                  }}>Cancel</button
                >
              </div>
            {/if}
            <input
              bind:this={workInput}
              class="work-input"
              data-testid="work-input"
              aria-label="Work key or name"
              onclick={(e) => e.stopPropagation()}
              placeholder={rowWork ? `Replace ${rowWork.key}…` : 'ABC-123 or a name'}
              bind:value={workDraft}
              onkeydown={onWorkKey}
              disabled={workBusy}
            />
            <button
              class="work-btn"
              data-testid="work-set"
              disabled={workBusy || workBlocked !== null || !workDraft.trim()}
              title={workBlocked ?? ''}
              onclick={setWork}
            >Set</button>
            {#if rowWork}
              <button
                class="work-btn"
                data-testid="work-reject"
                disabled={workBusy || workBlocked !== null}
                title={workBlocked ??
                  `${rowWork.key} is not this session's work; it will not be suggested again`}
                onclick={rejectWork}
              >Not {rowWork.key}</button>
              {#if rowWork.source === 'link' && sess.work}
                <button
                  class="work-btn"
                  data-testid="work-unlink"
                  disabled={workBusy || workBlocked !== null}
                  title={workBlocked ?? 'Remove the link (it may be recognised again)'}
                  onclick={clearWork}
                >Clear</button>
              {/if}
            {/if}
            <button
              class="work-btn"
              data-testid="work-name"
              disabled={workBusy || nameBlocked !== null}
              title={nameBlocked ?? 'Work with no ticket: give it a title (and a key if you like)'}
              onclick={openNameWork}
            >Name this work…</button>
            {#if localItem}
              <button
                class="work-btn"
                data-testid="work-rename"
                disabled={workBusy || renameWorkBlocked !== null}
                title={renameWorkBlocked ?? 'Rename this local work'}
                onclick={openRenameWork}
              >Rename…</button>
            {/if}
          </div>
        {/if}
        {#if sess.pending_form}
          <!-- Chat forms: the agent waits on a form; the row's own click opens the conversation. -->
          <span class="form-chip" data-testid="row-form-chip" title={sess.pending_form.title}>Form waiting</span>
        {/if}
        {#if answerView && promptBlocked === null && !compact}
          <!-- Claude is asking this row a question. The "Needs you" filter
               shows exactly these rows, so the answer belongs here and not
               only behind a click into the session.
               Hidden rather than disabled when this client may not drive the
               session (multi-user M1): the card is a set of answer buttons,
               and a watcher pressing one would only earn an E_FORBIDDEN. The
               `blocked` status chip above still says the session is waiting. -->
          <AnswerPrompt session={sess} view={answerView} compact />
        {/if}
        {#if starting}
          <!-- Step 5.14: ⌘N closes into the Pulse sequence on the new row;
               its worktree and tmux steps are done once the row is here, and
               the agent step lights when the agent reports a status. The
               only loader on the row: nothing else waits on it. -->
          <div class="starting-line" data-testid="row-starting">
            <PulseSteps pulse={startPulse} size={14} markOnly testid="row-pulse" />
            <span class="starting-text" role="status">{startText}</span>
          </div>
        {:else}
          {#if compact}
            <SessionRowMeta {sess} state={bucketState(triage.bucket)} {promptText} reason={blockedReason ?? waitingLine} />
          {:else if blockedReason}
            <!-- Comfortable has no meta line, but a Blocked row still says why
                 (the SessionRow component's line two: "Paused · weekly limit
                 on …") and offers its answers, in either density. -->
            <div class="blocked-line" data-testid="row-blocked-reason">{blockedReason}</div>
          {/if}
          {#if triage.bucket === 'account_limit'}
            <LimitActions
              {sess}
              resetsAt={$attentionFacts?.limited_accounts?.[sess.account_uuid ?? '']?.resets_at ?? null}
              accountName={(u) => accountLabel($accountByUuid.get(u))}
            />
          {/if}
          {#if !compact && $showRowDetails}
            <SessionRowDetails {sess} {nowSec} {secondaryName} />
          {/if}
        {/if}
      </div>
    {/if}
  {/if}
  {@render trailing?.()}
</div>
{#if nameDialog}
  <!-- `rename` mode is handed an item id and no session, so the dialog cannot
       ask the access half for itself: this row holds the session, so the answer
       goes in as a prop (multi-user M1, F2b). `name` mode narrows its own
       session list and ignores it. -->
  <NameWorkDialog
    target={nameDialog}
    accessBlocked={nameDialog.mode === 'rename' ? renameWorkBlocked : null}
    onclose={() => (nameDialog = null)}
  />
{/if}
{#if isRenaming && renameError}
  <p class="err inline-err">{renameError}</p>
{/if}
{#if rowMenu}
  <SessionRowMenu session={sess} x={rowMenu.x} y={rowMenu.y} onclose={() => (rowMenu = null)} />
{/if}

<style>
  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    line-height: 1;
    cursor: pointer;
    min-width: var(--control-h);
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }
  .icon-btn.small {
    padding: 0.1rem 0.35rem;
    font-size: var(--text-xs);
    min-width: var(--control-h);
    border-color: transparent;
  }
  .icon-btn.small:hover { border-color: var(--border); }
  .icon-btn.danger:hover { color: var(--danger); border-color: var(--danger); }

  /* Line 1's dot/badges/name sit near the row's vertical center; align the
     checkbox with that line instead of the two-line row's overall center
     (align-items: center on .sess-row would otherwise split the difference
     and visually float the box between the two lines). */
  .select-box { margin: 0; margin-top: 0.2rem; flex-shrink: 0; align-self: flex-start; }
  .sess-row.checked { outline: 1px solid var(--accent); }
  .sess-row.stuck { background: color-mix(in srgb, var(--danger) 6%, transparent); }

  .host-badge {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
    flex-shrink: 0;
  }

  .related-badge {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    background: var(--accent-soft);
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    flex-shrink: 0;
  }

  .review-badge,
  .shell-badge,
  .bg-badge { display: inline-flex; margin-left: var(--space-1); color: var(--fg-muted); }

  .err { color: var(--danger); font-size: var(--text-2xs); padding: 0.2rem 0; margin: 0; }
  .inline-err { padding-left: 1.6rem; font-size: var(--text-2xs); }

  .sess-row.stale {
    opacity: 0.6;
  }
  .sess-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: var(--text-2xs);
    padding: 0.22rem 0.4rem 0.22rem 1.4rem;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
    user-select: none;
  }
  .sess-row:hover { background: var(--bg-hover); }
  /* Selection is a bar as well as a tint (manual: SessionRow, accent-soft
     and the 2 px accent bar), so it does not rest on colour alone; with an
     org colour the bar sits just inside the 3 px org stripe. */
  .sess-row.selected { background: var(--accent-soft); box-shadow: inset 2px 0 0 var(--accent); }
  .sess-row[data-org-color] { box-shadow: inset 3px 0 0 var(--org-color); }
  .sess-row.selected[data-org-color] { box-shadow: inset 3px 0 0 var(--org-color), inset 5px 0 0 var(--accent); }
  .sess-row.renaming { background: var(--bg-pane); }
  /* The row is the app's primary navigation surface and is a tabbable
     treeitem. Without this a keyboard user tabbing the session list
     sees nothing move at all (WCAG 2.4.7). Drawn inward: the row is inside
     a scrolling list that clips an outset ring. */
  .sess-row:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .sess-row .row-actions {
    display: none;
    gap: 0.05rem;
  }
  .sess-row:hover .row-actions,
  .sess-row:focus-within .row-actions,
  .sess-row.selected .row-actions { display: flex; }

  /* Line 1's actions must never take flex width: reserving space for them
     permanently narrows the name, and NOT reserving space (the old rule)
     let them pop in at display:flex and squeeze the name to a sliver the
     instant the pointer entered. Take them out of flow entirely instead —
     absolutely positioned over the name's tail — with a solid strip in the
     row's current background plus a short fade so the truncated text reads
     cleanly right up to the overlay. */
  .sess-line1 .row-actions {
    position: absolute;
    right: 0;
    top: 50%;
    transform: translateY(-50%);
    padding-left: 0.15rem;
    background: var(--bg-pane);
  }
  .sess-line1 .row-actions::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    right: 100%;
    width: 1rem;
    background: linear-gradient(to right, transparent, var(--bg-pane));
  }
  .sess-row:focus-within .sess-line1 .row-actions,
  .sess-row:hover .sess-line1 .row-actions {
    background: var(--bg-hover);
  }
  .sess-row:focus-within .sess-line1 .row-actions::before,
  .sess-row:hover .sess-line1 .row-actions::before {
    background: linear-gradient(to right, transparent, var(--bg-hover));
  }
  .sess-row.selected .sess-line1 .row-actions {
    background: var(--accent-soft);
  }
  .sess-row.selected .sess-line1 .row-actions::before {
    background: linear-gradient(to right, transparent, var(--accent-soft));
  }

  .status-dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--fg-muted);
  }
  /* Colour never carries the state alone (7.2): the dot is labelled, and
     each state has its own shape. Running is a filled disc, frozen a ring,
     orphan a square, ghost a dashed ring. */
  .status-dot.status-running { background: var(--status-done); }
  .status-dot.status-frozen { background: transparent; box-shadow: inset 0 0 0 1.5px var(--status-working); }
  .status-dot.status-orphan { background: var(--status-failed); border-radius: var(--radius-xs); }
  .status-dot.status-ghost { background: transparent; border: 1.5px dashed var(--status-idle); box-sizing: border-box; opacity: 0.8; }
  .lost-at {
    font-size: var(--text-2xs);
    opacity: 0.6;
    margin-left: auto;
    padding-right: 0.25rem;
    white-space: nowrap;
  }

  /* The privacy badge (multi-user M1). Quiet on purpose: on a one-person
     fleet every row carries it, so it has to read as a label and not as an
     alert. */
  .lw-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    display: inline-block;
  }
  .lw-dot.tone-ok { background: var(--usage-ok); }
  .lw-dot.tone-pending { background: var(--usage-warn); }
  .lw-dot.tone-conflict,
  .lw-dot.tone-error { background: var(--usage-crit); }
  .lw-dot.tone-idle { border: 1.5px solid var(--fg-muted); box-sizing: border-box; }
  .lw-changes { font-size: var(--text-2xs); color: var(--usage-warn); }
  .privacy-chip {
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.05rem 0.28rem;
    border-radius: var(--radius-xs);
    border: 1px solid color-mix(in srgb, var(--fg-muted) 35%, transparent);
    color: var(--fg-muted);
    flex-shrink: 0;
    white-space: nowrap;
  }
  .form-chip {
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.05rem 0.28rem;
    border-radius: var(--radius-xs);
    border: 1px solid color-mix(in srgb, var(--usage-warn) 45%, transparent);
    color: var(--usage-warn);
    flex-shrink: 0;
    white-space: nowrap;
    align-self: flex-start;
  }
  .work-why {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    width: 100%;
    font-size: var(--text-2xs);
  }
  .why-link {
    display: flex;
    flex-wrap: wrap;
    gap: 0.15rem 0.4rem;
    align-items: baseline;
  }
  .why-key {
    font-family: var(--font-mono);
  }
  .why-what,
  .why-ev {
    color: var(--fg-muted);
  }
  .why-ev {
    flex-basis: 100%;
    padding-left: 0.6rem;
  }
  .why-actions {
    display: flex;
    gap: 0.3rem;
    flex-basis: 100%;
  }
  .why-trust {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    color: var(--fg-muted);
  }
  .cross-org {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    align-items: center;
    font-size: var(--text-2xs);
    color: var(--status-waiting);
  }
  .work-menu {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    align-items: center;
    padding: 0.2rem 0 0.1rem;
  }
  .work-input {
    flex: 1 1 8rem;
    min-width: 0;
    font-size: var(--text-2xs);
    padding: 0.1rem 0.3rem;
  }
  .work-btn {
    font-size: var(--text-2xs);
    padding: 0.05rem 0.35rem;
    white-space: nowrap;
  }
  .sess-lines { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 0.1rem; }
  .sess-line1 { position: relative; display: flex; align-items: center; gap: 0.4rem; min-width: 0; }
  .sess-line1 .sess-name { flex: 1; }
  .starting-line {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    padding-left: 0.85rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .blocked-line {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-left: 0.85rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .starting-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Redesign step 3.6: the chip strip and the Compact row. */
  .chips { display: contents; }
  /* A 13px name line, a 2xs meta line and the row's padding: 40px
     (COMPACT_ROW_PX), so 20 rows fit a 1080p window
     (SessionRowDensity.test.ts measures it). */
  .sess-row.compact { box-sizing: border-box; min-height: calc(var(--text-sm-lh) + var(--text-2xs-lh) + var(--space-2)); }
  .acct { display: contents; }
  .sess-row.compact .chips,
  .sess-row.compact .acct { display: none; }
  .sess-row.compact:hover .chips,
  .sess-row.compact:focus-within .chips,
  .sess-row.compact.selected .chips,
  .sess-row.compact:hover .acct,
  .sess-row.compact:focus-within .acct,
  .sess-row.compact.selected .acct { display: contents; }
  .sess-row.compact .sess-name {
    flex: 1 1 auto;
    font-family: var(--font-sans);
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--fg);
  }
  .sess-age {
    flex-shrink: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .label-chip {
    flex: 0 0 auto;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    line-height: 14px;
    white-space: nowrap;
  }
  .sess-name {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rename-input {
    flex: 1;
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    padding: 0.1rem 0.3rem;
    border: 1px solid var(--accent);
    background: var(--bg);
    color: var(--fg);
    border-radius: var(--radius-xs);
    outline: none;
    min-width: 0;
  }
  .rename-input:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: var(--ring-offset); }

</style>
