<!--
  Work → Missions' room (Thread room for Work, 2026-10-15; boards
  W1Missions and W2Plan, the look Control got on 2026-10-14). The mission
  picked in the sidebar's Missions tab (WorkMissions), in the right column:

  - centre, one column of at most 760 px: the header (name, state, the
    autonomy and spend line, the state moves: Pause / Start / Resume inline,
    Complete, Mark failed and Cancel under ⋯ behind a confirm, Edit), the
    task graph card, then the mission's log as a chat: its events as quiet
    lines, then what needs a person as cards — the planner's proposals
    (Accept all, Review one by one, Ask the planner again), the confirm
    queue, a stuck mission's triage — the next step with its button, and
    Finish when it ended. "Steer <mission>…" at the bottom hands the text
    to Control's agent, prefixed with the mission, and leaves a receipt;
  - right, the panel, resizable like Control's: Session (a live session of
    the mission, with the strip of its others), Plan (goal, done when, not
    in scope, autonomy and the grant, planner and brakes, the tasks, Import
    a plan, Finish, Delete; Edit opens here), Runs, Log and Repos.

  With no mission picked, a starter: New mission, Plan from a task. The New
  mission form shows here too. Text a person or a tracker wrote (names,
  goals, task titles) is rendered as text, never as markup.
-->
<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import Resizer from './Resizer.svelte';
  import Skeleton from './states/Skeleton.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import OrbitMark from './kit/OrbitMark.svelte';
  import QuestionCard from './kit/QuestionCard.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import MissionFinish from './MissionFinish.svelte';
  import MissionCompare from './MissionCompare.svelte';
  import MissionTriage from './MissionTriage.svelte';
  import MissionTasks from './MissionTasks.svelte';
  import TaskGraphCard from './TaskGraphCard.svelte';
  import SessionFocus from './SessionFocus.svelte';
  import ReleaseNote from './ReleaseNote.svelte';
  import PlanImportForm from './PlanImportForm.svelte';
  import { tablistKeys } from './tablist_keys';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { orgs } from './orgs';
  import { projects } from './projects';
  import { hosts } from './hosts';
  import { myPersonId } from './access';
  import { sessions as sessionRows, type SessionRow } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { push } from './toasts';
  import { shortAge, timeAgo } from './session_status';
  import { onWorkChangedDebounced, decideWorkProposal } from './work';
  import { leave } from './destination';
  import { NEWER_HUB, isOlderHub, openTask, readErrorText as rawErrorText } from './work_view';
  import { importLine, importMissionPlan, type PlanRow } from './plan_import';
  import { graphFromMission } from './task_graph_card';
  import { toneOf } from './mission_graph';
  import { askAgent, operatorRow, operatorSession, refreshOperator } from './operator';
  import { readPref, writePref } from './prefs';
  import { sessionState } from './handoffs';
  import { openControl } from './control';
  import { outbox, receipt as outboxReceipt } from './outbox';
  import type { NextStep } from './mission_triage';
  import type { IpcError, Result } from './result';
  import { parkedHeld } from './composer_hold';
  import { budgetProblem, microsOf } from './routines';
  import { formKeys } from './forms/form_frame';
  import {
    acceptWorkProposals,
    autonomyWords,
    brakesLine,
    cardLine,
    createMission,
    decideMissionCard,
    deleteMission,
    doneWhenRows,
    dollars,
    eventSentence,
    finalMoveQuestion,
    getMission,
    grantMission,
    grantSignLabel,
    grantSpanNotice,
    grantWouldHelp,
    isFinal,
    missionTaskId,
    moveLabel,
    ownerLine,
    planMission,
    plannerWaiting,
    plannerLimitWait,
    limitWaitSentence,
    plannerError,
    plannerLine,
    plannerRefusal,
    policyWith,
    progressLabel,
    retryWorkItem,
    revokeMissionGrant,
    runStateLabel,
    runsOf,
    setMissionRepo,
    setMissionState,
    splitMoves,
    startMissionWave,
    stateLabel,
    stepKey,
    stepLine,
    undoWorkAccept,
    untilWords,
    updateMission,
    waitsOnPerson,
    wakeLabel,
    withoutConfigKeys,
    POLICY_DEFAULT_PARALLEL,
    POLICY_MAX_PARALLEL,
    POLICY_MIN_WAKE_SECS,
    MISSION_LEVELS,
    type FinishSession,
    type HumanError,
    type MissionCard,
    type MissionDetail,
  } from './missions';
  import {
    MISSION_PANEL_TABS,
    defaultPanelTab,
    dragMissionRight,
    loadMissionList,
    missionList,
    missionTone,
    missionPanelOpen,
    missionPanelTab,
    missionRightPx,
    missionSessionId,
    openMissionSession,
    proposalSummary,
    roomComparing,
    roomCreating,
    roomEvents,
    roomMissionId,
    sendSteer,
    showMissionPanel,
    startNewMission,
    statsLine,
    steerBlocked,
    steerBusy,
    steerPrefix,
    steerReceipts,
    type MissionPanelTab,
  } from './mission_room';

  /** An error's text for a notice, with no settings key in it (step 1.3). */
  const readErrorText = (e: IpcError) => withoutConfigKeys(rawErrorText(e));

  const missions = $derived($missionList.missions);
  let detail = $state.raw<MissionDetail | null>(null);
  let notice = $state<string | null>(null);
  /** Where the notice is drawn: beside the press that raised it. `panel`
   *  for the right panel's own (the Plan tab's Save, Sign, Import, Delete,
   *  a task edit), `next` for Start wave and Go (the Next block sits at the
   *  end of the centre column, and its top is out of sight by then),
   *  `centre` at the top for the rest. */
  let noticeAt = $state<'centre' | 'panel' | 'next'>('centre');
  /** Review w17-4: the last re-read of the open mission failed; what was
   *  loaded stays on screen. */
  let refreshError = $state<IpcError | null>(null);
  let busy = $state(false);
  /** The last planner press that failed, in words (step 1.3). */
  let plannerFailure = $state<HumanError | null>(null);
  /** `plannerFailure` is a usage-limit wait, not a failure. */
  let plannerIsWait = $state(false);
  let plannerDetails = $state(false);

  const mission = $derived(detail?.mission ?? null);
  const plan = $derived(detail?.plan ?? null);
  /** The header's autonomy in words: "Runs at L1 · L3 asked · L1 ceiling". */
  const autonomy = $derived(plan ? autonomyWords(plan.autonomy) : null);
  /** Its runs and its idle time (lanes design L0). */
  const statsText = $derived(statsLine(detail?.stats));
  const pressable = $derived((plan?.steps ?? []).filter((s) => s.kind !== 'ask'));
  const openCards = $derived((plan?.cards ?? []).filter((c) => c.state === 'open' && !hiddenCards.includes(c.id)));
  const runs = $derived(detail ? runsOf(detail) : []);
  const proposals = $derived(detail ? proposalSummary(detail) : { ids: [], waves: 0, titles: [] });
  const graph = $derived(detail ? graphFromMission(detail) : null);
  /** "9 tasks · 3 waves". */
  const proposalsLine = $derived(
    `${proposals.ids.length} ${proposals.ids.length === 1 ? 'task' : 'tasks'}${proposals.waves > 1 ? ` · ${proposals.waves} waves` : ''}`,
  );
  const events = $derived(detail ? roomEvents(detail) : []);
  const limitWait = $derived(detail ? plannerLimitWait(detail) : null);
  const itemById = $derived(new Map((detail?.items ?? []).map((i) => [i.id, i])));
  const needsYou = $derived(!!detail && !!mission && !isFinal(mission.state) && (!!mission.waiting_on || waitsOnPerson(detail)));

  const saveBlocked = $derived(!!hubActionBlocked('save_mission', $hubStatus, $hubConnection));
  const changeBlocked = $derived(!!hubActionBlocked('set_mission_state', $hubStatus, $hubConnection));
  const importBlocked = $derived(!!hubActionBlocked('import_mission_plan', $hubStatus, $hubConnection));
  const mayChange = $derived(!!detail?.may_change && !!mission && !isFinal(mission.state));
  const split = $derived(mission ? splitMoves(mission.state) : { inline: [], menu: [] });

  function personName(id: number): string | null {
    for (const o of $orgs) {
      const p = (o.members ?? []).find((x) => x.person_id === id);
      if (p) return p.display_name || p.name;
    }
    return null;
  }
  const owner = $derived(
    detail
      ? ownerLine(
          detail.mission,
          $myPersonId,
          personName,
          (id) => $orgs.find((o) => o.id === id)?.name ?? null,
        )
      : null,
  );

  function titleOf(id: number): string {
    const it = itemById.get(id);
    if (it) return it.title;
    const o = (detail?.graph?.outside ?? []).find((x) => x.id === id);
    return o ? (o.key ?? o.title) : `#${id}`;
  }

  // ── Loading ──

  /** The centre column's scroller, while a mission is on show. */
  let scroller = $state<HTMLDivElement>();
  /** The person has not scrolled up from the end of the log. */
  let pinnedToEnd = true;
  function onScrolled() {
    if (scroller) pinnedToEnd = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 32;
  }

  /** The mission `open` last set up the room for. */
  let openedId: number | null = null;
  // Bumped per request and by every open: a late answer for a mission no
  // longer open is dropped (review r07).
  let detailSeq = 0;

  async function loadDetail(id: number) {
    const mine = ++detailSeq;
    const r = await getMission(id);
    if (mine !== detailSeq || get(roomMissionId) !== id) return;
    if (r.ok && r.value?.mission) {
      const first = detail?.mission.id !== id;
      // The log reads as a chat: what needs a person (the cards, Next) is
      // at its end. Opening lands there, and a room left at the end stays
      // there as events arrive; one scrolled up is left alone.
      const toEnd = first || pinnedToEnd;
      detail = r.value;
      refreshError = null;
      if (first) missionPanelTab.set(defaultPanelTab(r.value));
      if (toEnd) {
        await tick();
        if (scroller) scroller.scrollTop = scroller.scrollHeight;
      }
    } else if (!r.ok && r.error.code !== 'E_NOTFOUND' && detail?.mission.id === id) {
      // Review w17-4: a failed background re-read is not "the mission is
      // gone": the room, an open goal edit and a half-typed answer stay.
      // E_NOTFOUND is (the hub answers a deleted or no-longer-visible
      // mission with it, never with `ok` and no mission): review w30-1.
      refreshError = r.error;
    } else {
      notice =
        r.ok || r.error.code === 'E_NOTFOUND' ? 'That mission is not in the fleet any more.' : readErrorText(r.error);
      detail = null;
      roomMissionId.set(null);
      openedId = null;
    }
  }

  /** Re-read the list (both columns) and the open mission. */
  async function load() {
    await loadMissionList();
    const id = get(roomMissionId);
    const l = get(missionList);
    if (id != null && l.loaded && !l.error && !l.missions.some((m) => m.id === id)) {
      detailSeq++;
      detail = null;
      roomMissionId.set(null);
      openedId = null;
      return;
    }
    if (id != null) await loadDetail(id);
  }

  /** Set the room up for `id`, every per-mission choice reset. */
  async function open(id: number | null) {
    openedId = id;
    detailSeq++;
    lastAccepted = [];
    editing = false;
    confirmDelete = false;
    moreOpen = false;
    confirmMove = null;
    notice = null;
    noticeAt = 'centre';
    pinnedToEnd = true;
    refreshError = null;
    plannerFailure = null;
    plannerIsWait = false;
    // A refused Complete's override is that mission's (review r8): it never
    // carries over to the next one.
    ciRed = false;
    importOpen = false;
    importText = '';
    importResult = null;
    importUnknown = [];
    reviewing = false;
    reviewAt = 0;
    granting = false;
    grantUntilDone = false;
    grantHosts = [];
    earlierShown = false;
    steerText = '';
    missionSessionId.set(null);
    if (id == null) {
      detail = null;
      return;
    }
    if (detail?.mission.id !== id) detail = null;
    await loadDetail(id);
  }

  $effect(() => {
    const id = $roomMissionId;
    untrack(() => {
      if (id !== openedId) void open(id);
    });
  });

  /** Run one write; on success re-read the list and the open mission.
   *  `where` says which column pressed, so a refusal lands beside the press. */
  async function act<T>(p: Promise<Result<T>>, where: 'centre' | 'panel' | 'next' = 'centre'): Promise<T | null> {
    busy = true;
    notice = null;
    noticeAt = where;
    ciRed = false;
    try {
      const r = await p;
      if (!r.ok) {
        notice = readErrorText(r.error);
        ciRed = r.error.code === 'E_CI_FAILING';
        if (r.error.code === 'E_CONFLICT') await load();
        return null;
      }
      await load();
      return r.value;
    } finally {
      busy = false;
    }
  }

  /** `act` for the panel's own presses (the tasks block). */
  function panelAct<T>(p: Promise<Result<T>>): Promise<T | null> {
    return act(p, 'panel');
  }

  /** The mission's member tasks: every item but its root. */
  const members = $derived((detail?.items ?? []).filter((i) => i.id !== mission?.root_item_id));
  /** The Next block (the steps and Start wave) is on show. */
  const nextShown = $derived(
    (plan?.steps ?? []).length > 0 || (mayChange && !!mission && mission.state === 'active' && pressable.length > 0),
  );
  /** Nothing to run yet and nothing proposed: the centre says what comes next. */
  const nothingYet = $derived(members.length === 0 && proposals.ids.length === 0 && (plan?.steps ?? []).length === 0);

  // ── Opening tasks and sessions ──

  /** Open a member task as the rest of Work does: select it, show its
   *  detail (the room steps aside for it; the sidebar keeps the list). */
  function openItem(itemId: number) {
    const links = (detail?.sessions ?? []).filter((s) => s.item_id === itemId).map((s) => ({ session_id: s.session_id, state: 'active' }));
    openTask(missionTaskId(itemId), links);
    leave('missions');
  }

  /** One of its sessions, in the panel's Session tab, in place. */
  function openSession(s: FinishSession) {
    if (!$sessionRows.some((r) => r.id === s.session_id)) {
      push({ kind: 'info', message: `${s.tmux_name} is gone: the session is no longer in the fleet.` });
      return;
    }
    openMissionSession(s.session_id);
  }

  // The Session tab: the mission's live sessions that are in the fleet, and
  // the one in focus (the one picked, else the first).
  const liveSessions = $derived(detail?.sessions ?? []);
  const liveRows = $derived(
    liveSessions
      .map((s) => $sessionRows.find((r) => r.id === s.session_id))
      .filter((r): r is SessionRow => !!r),
  );
  const focusRow = $derived(
    ($missionSessionId != null ? $sessionRows.find((r) => r.id === $missionSessionId) : undefined) ?? liveRows[0] ?? null,
  );
  const focusLink = $derived(focusRow ? liveSessions.find((s) => s.session_id === focusRow.id) : undefined);

  function openFull(row: SessionRow) {
    const link = liveSessions.find((s) => s.session_id === row.id);
    selectSessionExplicitly(row, link ? { task: missionTaskId(link.item_id) } : {});
  }

  // ── The right panel ──

  let rawRight = $state<number | null>(null);
  function onResizeRight(delta: number) {
    rawRight = dragMissionRight(rawRight ?? $missionRightPx, delta);
  }
  function endRight() {
    rawRight = null;
  }
  const grid = $derived(`minmax(0, 1fr) 4px ${$missionRightPx}px`);
  /** A mission is on show (not the starter, not the New mission form). */
  const showing = $derived(!!mission && !!detail && !$roomCreating && !$roomComparing);
  const tabCount = (id: MissionPanelTab): number =>
    id === 'session' ? liveSessions.length : id === 'runs' ? runs.length : id === 'repos' ? (mission?.repos ?? []).length : 0;

  // ── Editing (in the Plan tab) ──

  let editing = $state(false);
  let editGoal = $state('');
  let editNonGoals = $state('');
  let editDoneWhen = $state('');
  let editLevel = $state(0);
  let editMode = $state('finite');
  /** Runs open at once, and a continuous mission's wake interval in minutes
   *  ('' = no timer) (step 1.8). */
  let editParallel = $state(POLICY_DEFAULT_PARALLEL);
  /** How its tasks run (lanes design L3): `item` or `lane`. */
  let editExecution = $state<'item' | 'lane'>('item');
  let editLaneTestAt = $state<'item' | 'lane'>('item');
  let editLanePr = $state<'off' | 'draft'>('off');
  let editWakeMins = $state<number | null>(null);
  const minWakeMins = POLICY_MIN_WAKE_SECS / 60;
  const wakeBad = $derived(editMode === 'continuous' && editWakeMins != null && !(editWakeMins >= minWakeMins));
  const parallelBad = $derived(!(Number.isInteger(editParallel) && editParallel >= 1 && editParallel <= POLICY_MAX_PARALLEL));

  function startEdit() {
    if (!mission) return;
    editGoal = mission.goal;
    editNonGoals = mission.non_goals ?? '';
    editDoneWhen = (mission.done_when ?? []).join('\n');
    editLevel = mission.level;
    editMode = mission.mode;
    editParallel = mission.policy?.max_parallel ?? POLICY_DEFAULT_PARALLEL;
    editExecution = mission.policy?.execution ?? 'item';
    editLaneTestAt = mission.policy?.lane_test_at ?? 'item';
    editLanePr = mission.policy?.lane_pr ?? 'off';
    const wake = mission.policy?.wake_every_secs;
    editWakeMins = wake ? Math.round(wake / 60) : null;
    editing = true;
    showMissionPanel('plan');
  }

  async function saveEdit() {
    if (!mission || wakeBad || parallelBad) return;
    const wakeSecs = editMode === 'continuous' && editWakeMins != null ? Math.round(editWakeMins * 60) : null;
    const m = await act(
      updateMission(
        mission.id,
        {
          goal: editGoal,
          non_goals: editNonGoals,
          done_when: doneWhenRows(editDoneWhen),
          level: editLevel,
          mode: editMode,
          policy: policyWith(mission.policy, {
            max_parallel: editParallel,
            wake_every_secs: wakeSecs,
            execution: editExecution,
            lane_test_at: editLaneTestAt,
            lane_pr: editLanePr,
          }),
        },
        mission.version,
      ),
      'panel',
    );
    if (m) editing = false;
  }

  // ── The state moves (parity P19: the ending ones ask first, from ⋯) ──

  let moreOpen = $state(false);
  let confirmMove = $state<string | null>(null);
  let moreEl = $state<HTMLElement>();

  /** The last Complete was refused for red CI on the mission's PRs: offer
   *  a person's override under the notice. */
  let ciRed = $state(false);

  async function move(to: string, ciOverride = false) {
    if (!mission) return;
    await act(setMissionState(mission.id, to, mission.version, ciOverride));
  }

  function pickFinal(to: string) {
    moreOpen = false;
    confirmMove = to;
  }

  async function confirmFinal() {
    const to = confirmMove;
    confirmMove = null;
    if (to) await move(to);
  }

  function moreKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      moreOpen = false;
      moreEl?.querySelector<HTMLElement>('[data-testid="mission-more"]')?.focus();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const all = Array.from(moreEl?.querySelectorAll<HTMLElement>('[role=menuitem]:not(:disabled)') ?? []);
    if (!all.length) return;
    e.preventDefault();
    const i = all.indexOf(document.activeElement as HTMLElement);
    all[(i + (e.key === 'ArrowDown' ? 1 : all.length - 1)) % all.length].focus();
  }

  $effect(() => {
    if (!moreOpen) return;
    moreEl?.querySelector<HTMLElement>('[role=menuitem]:not(:disabled)')?.focus();
    const away = (e: PointerEvent) => {
      if (moreEl && !moreEl.contains(e.target as Node)) moreOpen = false;
    };
    window.addEventListener('pointerdown', away, true);
    return () => window.removeEventListener('pointerdown', away, true);
  });

  // ── The loop (orchestration O4–O8) ──

  /** A short report of what a press did, failures first. */
  function reportSteps(results: { ok: boolean; detail: string }[] | undefined) {
    const bad = (results ?? []).filter((r) => !r.ok);
    if (bad.length > 0) notice = bad.map((r) => r.detail).join(' · ');
    else if ((results ?? []).length === 0) notice = 'Nothing to start right now.';
  }

  async function startWave(step?: string) {
    if (!mission) return;
    const out = await act(startMissionWave(mission.id, step), 'next');
    if (out) reportSteps(out.results);
  }

  async function retry(itemId: number) {
    const out = await act(retryWorkItem(itemId));
    if (out && !out.ok) notice = out.detail;
  }

  /** Redesign 9.10: a step picked on the stuck card, through the action a
   *  person already has. Nothing is completed, cancelled or verified here. */
  function triageStep(step: NextStep) {
    if (step === 'retry') {
      const failed = (detail?.graph?.nodes ?? [])
        .filter((n) => n.state === 'failed')
        .sort((a, b) => (b.attempt?.task_id ?? 0) - (a.attempt?.task_id ?? 0))[0];
      if (failed) void retry(failed.item_id);
      else void askPlanner();
    } else if (step === 'split') {
      void askPlanner();
    } else if (step === 'give_up') {
      pickFinal('cancelled');
    } else {
      document.querySelector('[data-testid="mission-next"]')?.scrollIntoView?.({ block: 'nearest' });
    }
  }

  async function askPlanner() {
    if (!mission) return;
    plannerFailure = null;
    plannerIsWait = false;
    plannerDetails = false;
    busy = true;
    notice = null;
    try {
      const r = await planMission(mission.id);
      if (!r.ok) {
        plannerFailure = isOlderHub(r.error)
          ? { title: "The planner couldn't run", text: NEWER_HUB, details: `${r.error.code} · ${r.error.message}` }
          : plannerError(r.error);
        if (r.error.code === 'E_CONFLICT') await load();
        return;
      }
      if (r.value.refused) plannerFailure = plannerRefusal(r.value.refused);
      else if (r.value.waiting) {
        plannerFailure = plannerWaiting(r.value.waiting);
        plannerIsWait = true;
      }
      await load();
    } finally {
      busy = false;
    }
  }

  // The planner's proposals: all at once (with Undo while nothing touched
  // them, `store::ACCEPT_UNDO_SECS`), or one by one.
  let lastAccepted = $state<number[]>([]);
  let reviewing = $state(false);
  let reviewAt = $state(0);
  const reviewId = $derived(reviewing ? (proposals.ids[reviewAt] ?? null) : null);

  async function acceptAll() {
    const ids = proposals.ids;
    if (await act(acceptWorkProposals(ids))) {
      lastAccepted = ids;
      reviewing = false;
    }
  }

  async function undoAccept() {
    const ids = lastAccepted;
    if (await act(undoWorkAccept(ids))) lastAccepted = [];
  }

  async function reviewOne(accept: boolean) {
    const id = reviewId;
    if (id == null) return;
    // The decided one leaves the list: the next one takes its place. An
    // accept one by one is undone as Accept all's is (ACCEPT_UNDO_SECS):
    // a slipped Accept is one press away, not a trip to Work.
    const out = await act(decideWorkProposal(id, accept));
    if (out && accept) lastAccepted = [...lastAccepted, id];
    if (reviewAt >= proposals.ids.length) reviewing = false;
  }

  function reviewSkip() {
    reviewAt += 1;
    if (reviewAt >= proposals.ids.length) reviewing = false;
  }

  // G2.5: a mission's question answered in its room, as the session's
  // question card is: its options, your own words, or Skip.
  let answers = $state<Record<number, string>>({});
  let ownWords = $state<Record<number, boolean>>({});
  function askOptions(c: MissionCard): string[] {
    const o = (c.payload as { options?: unknown } | null | undefined)?.options;
    return Array.isArray(o) ? o.filter((x): x is string => typeof x === 'string' && x.trim() !== '').slice(0, 6) : [];
  }
  async function answerWith(c: MissionCard, option: string) {
    answers = { ...answers, [c.id]: option };
    await decide(c, true);
  }

  async function decide(c: MissionCard, ok: boolean) {
    const note = c.kind === 'ask' && ok ? (answers[c.id] ?? '').trim() : undefined;
    if (c.kind === 'ask' && ok && !note) {
      notice = 'Write an answer first.';
      return;
    }
    const out = await act(decideMissionCard(c.id, ok, note));
    if (out?.state === 'refused') notice = out.note ?? 'Refused';
    // Said under the card that was pressed, too: the room's notice sits at
    // its top, out of sight from a card further down, and the press looked
    // like it did nothing.
    cardNote = notice !== null ? { id: c.id, text: notice } : null;
  }
  let cardNote = $state<{ id: number; text: string } | null>(null);
  /** Cards this person cannot act on and hid, by id (per window). */
  let hiddenCards = $state<number[]>([]);

  // The grant: what the loop may do by itself, for how long, on what.
  let granting = $state(false);
  let grantLevel = $state(2);
  let grantHours = $state(8);
  let grantBudget = $state('');
  /** Until the mission completes (a finite mission's), bounded by the budget. */
  let grantUntilDone = $state(false);
  /** Hosts the grant lets the loop run on; none ticked: any host. */
  let grantHosts = $state<string[]>([]);

  function focusNow(el: HTMLElement): void {
    el.focus();
  }

  /** What is wrong with the grant's Budget, said under the field: "5,50"
   *  is $5.50 and a typo is refused rather than signed as no cap. */
  const grantBudgetBad = $derived(budgetProblem(grantBudget));

  async function grant() {
    if (!mission || grantBudgetBad) return;
    const micros = microsOf(grantBudget);
    const cents = micros === undefined ? 0 : Math.round(micros / 10_000);
    const untilDone = grantUntilDone && mission.mode === 'finite';
    if (untilDone && !(cents > 0)) {
      notice = 'A grant until the mission completes needs a budget.';
      noticeAt = 'panel';
      return;
    }
    const out = await act(
      grantMission(mission.id, {
        level: grantLevel,
        ...(untilDone ? { until_done: true } : { hours: grantHours }),
        ...(cents > 0 ? { budget_cents: cents } : {}),
        ...(grantHosts.length > 0 ? { hosts: [...grantHosts] } : {}),
      }),
      'panel',
    );
    if (out) {
      granting = false;
      grantHosts = [];
      grantUntilDone = false;
      // A hub before grants until done drops the flag and signs its default
      // hours (review r8): say so rather than let it pass as asked.
      notice = grantSpanNotice(untilDone, out);
    }
  }

  async function revokeGrant() {
    if (mission) await act(revokeMissionGrant(mission.id), 'panel');
  }

  // ── Import a plan: a markdown step table read into the mission's tasks ──

  // List or Graph: the choice outlives the mission (a pref).
  const isTasksView = (v: unknown): v is 'list' | 'graph' => v === 'list' || v === 'graph';
  let tasksView = $state<'list' | 'graph'>(readPref<'list' | 'graph'>('work.missions.view', 'list', isTasksView));
  $effect(() => writePref('work.missions.view', tasksView));
  let importOpen = $state(false);
  let importText = $state('');
  let importResult = $state<string | null>(null);
  let importUnknown = $state<string[]>([]);
  let importRows = $state<PlanRow[] | null>(null);

  async function runImport() {
    if (!mission || !importRows || importRows.length === 0) return;
    const out = await act(importMissionPlan(mission.id, importRows), 'panel');
    if (out) {
      importResult = importLine(out);
      importUnknown = out.unknown_needs ?? [];
      importText = '';
      importOpen = false;
      tasksView = 'graph';
    }
  }

  // ── Repos ──

  let repoPick = $state<number | ''>('');
  let repoRole = $state('');
  const repoChoices = $derived(
    $projects.map((p) => p.project).filter((p) => !(mission?.repos ?? []).some((r) => r.project_id === p.id)),
  );

  async function addRepo() {
    if (!mission || repoPick === '') return;
    if (await act(setMissionRepo(mission.id, repoPick, true, repoRole), 'panel')) {
      repoPick = '';
      repoRole = '';
    }
  }

  // ── Delete ──

  let confirmDelete = $state(false);
  async function remove() {
    if (!mission) return;
    if (await act(deleteMission(mission.id), 'panel')) {
      roomMissionId.set(null);
      void open(null);
    }
  }

  // ── New mission (G2.5: the planner drafts the first tasks only when
  //    asked; a plan can be imported into it) ──

  let newName = $state('');
  let newGoal = $state('');
  let newPlanner = $state(false);
  let newImportOpen = $state(false);
  let newImportText = $state('');
  let newImportRows = $state<PlanRow[] | null>(null);

  async function create() {
    const name = newName.trim();
    const goal = newGoal.trim();
    if (!name || !goal) {
      notice = 'A mission needs a name and a goal.';
      return;
    }
    if (newImportOpen && newImportText.trim() && !newImportRows) {
      notice = 'The plan is not ready to import: see the rows above.';
      return;
    }
    const m = await act(createMission({ name, goal }));
    if (!m) return;
    const rows = newImportOpen ? newImportRows : null;
    const planner = newPlanner;
    newName = '';
    newGoal = '';
    newPlanner = false;
    newImportOpen = false;
    newImportText = '';
    roomCreating.set(false);
    roomMissionId.set(m.id);
    await open(m.id);
    if (rows && rows.length > 0) {
      const out = await act(importMissionPlan(m.id, rows));
      if (out) {
        importResult = importLine(out);
        importUnknown = out.unknown_needs ?? [];
        tasksView = 'graph';
      }
    }
    // The planner drafts the first tasks as cards to confirm.
    if (planner) await askPlanner();
  }

  // ── The timeline ──

  /** The log shows its last few lines until asked for the rest. */
  const EVENTS_SHOWN = 30;
  let earlierShown = $state(false);
  const shownEvents = $derived(earlierShown ? events : events.slice(-EVENTS_SHOWN));

  // ── Steer: the composer hands the text to Control ──

  let steerText = $state('');
  const blocked = $derived(steerBlocked($operatorRow));
  const outboxStore = outbox.store;
  const operatorOutboxBusy = $derived(
    ($outboxStore.msgs[$operatorRow?.id ?? -1] ?? []).some((m) => m.state === 'waiting' || m.state === 'sending' || m.state === 'sent'),
  );
  const steerWait = $derived(blocked ? null : steerBusy($operatorRow, operatorOutboxBusy, $parkedHeld.length));
  const myReceipts = $derived(mission ? $steerReceipts.filter((r) => r.missionId === mission.id) : []);

  function steer() {
    const text = steerText.trim();
    const row = get(operatorRow);
    if (!mission || !text || !row || steerBlocked(row) || steerWait) return;
    sendSteer(mission, text, row);
    steerText = '';
  }

  function steerKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      steer();
    }
  }

  function receiptLabel(key: string, operatorId: number): string {
    const m = ($outboxStore.msgs[operatorId] ?? []).find((x) => x.id === key);
    return m ? outboxReceipt(m, false, Date.now()).label : 'Sent';
  }

  let stopWatching: (() => void) | null = null;
  onMount(() => {
    // Who the composer would send to: read, never woken, from here.
    if (!get(operatorSession)) refreshOperator().catch(() => {});
    stopWatching = onWorkChangedDebounced(
      () => {
        const id = get(roomMissionId);
        if (id != null) void loadDetail(id);
      },
      () => 500,
      () => 3000,
    );
  });
  onDestroy(() => stopWatching?.());
</script>

<section
  class="room"
  aria-label="Mission"
  data-testid={showing ? 'mission-detail' : 'mission-room'}
  style:grid-template-columns={showing && $missionPanelOpen ? grid : 'minmax(0, 1fr) 0px 0px'}
>
  <div class="centre">
    {#if $roomComparing}
      <MissionCompare />
    {:else if $roomCreating}
      <div class="scroll">
        <div class="col">
          {#if notice}<p class="notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
          <form class="create" data-testid="mission-create-form" use:formKeys onsubmit={(e) => (e.preventDefault(), void create())}>
            <h2 class="name">New mission</h2>
            <!-- The name takes focus as the form opens: New mission is the
                 press that asked for it, and the next thing is typing. -->
            <input placeholder="Name" aria-label="Name" bind:value={newName} use:focusNow data-testid="mission-new-name" />
            <textarea
              rows="3"
              placeholder="Goal: what is true when it is done"
              aria-label="Goal"
              bind:value={newGoal}
              data-testid="mission-new-goal"
            ></textarea>
            <label class="check"
              ><input type="checkbox" bind:checked={newPlanner} data-testid="mission-new-planner" /> Let the planner draft the first tasks
              <span class="muted small">(you confirm each one)</span></label
            >
            {#if newImportOpen}
              <PlanImportForm bind:text={newImportText} bind:ready={newImportRows} testid="mission-new-import" />
            {:else}
              <button class="btn btn--quiet start" type="button" data-testid="mission-new-import-open" onclick={() => (newImportOpen = true)}
                >Import a plan…</button
              >
            {/if}
            <div class="row">
              <button class="btn btn--primary" type="submit" disabled={busy || saveBlocked} data-testid="mission-create">Create</button>
              <button class="btn btn--quiet" type="button" onclick={() => roomCreating.set(false)}>Cancel</button>
            </div>
          </form>
        </div>
      </div>
    {:else if mission && detail}
      <header class="head" data-testid="mission-head">
        <div class="title-row">
          <h2 class="name" data-testid="mission-name">{mission.name}</h2>
          <StatusChip
            state={missionTone(mission, detail)}
            label={needsYou ? 'Needs you' : stateLabel(mission.state, detail.phase)}
            testid="mission-state"
          />
          <span class="grow"></span>
          {#if mayChange}
            {#each split.inline as to (to)}
              <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} data-testid="mission-move-{to}" onclick={() => void move(to)}
                >{moveLabel(mission.state, to)}</button
              >
            {/each}
            <button class="btn btn--quiet" type="button" disabled={busy || saveBlocked} data-testid="mission-edit" onclick={startEdit}>Edit</button>
            {#if split.menu.length > 0}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <span class="more" bind:this={moreEl} onkeydown={moreKey}>
                <button
                  class="btn btn--quiet"
                  type="button"
                  aria-label="More mission actions"
                  aria-haspopup="menu"
                  aria-expanded={moreOpen}
                  disabled={busy || changeBlocked}
                  data-testid="mission-more"
                  onclick={() => (moreOpen = !moreOpen)}>⋯</button
                >
                {#if moreOpen}
                  <span class="more-menu" role="menu" aria-label="Mission actions" data-testid="mission-more-menu">
                    {#each split.menu as to (to)}
                      <button
                        type="button"
                        role="menuitem"
                        class="mi"
                        class:danger={to !== 'completed'}
                        data-testid="mission-menu-{to}"
                        onclick={() => pickFinal(to)}>{moveLabel(mission.state, to)}…</button
                      >
                    {/each}
                  </span>
                {/if}
              </span>
            {/if}
          {/if}
          {#if !$missionPanelOpen}
            <button
              type="button"
              class="btn btn--quiet btn--icon"
              data-testid="mission-panel-open"
              title="Show the panel: Session, Plan, Runs, Log, Repos"
              aria-label="Show the panel"
              onclick={() => missionPanelOpen.set(true)}>‹</button
            >
          {/if}
        </div>
        <p class="meta">
          {#if mission.mode === 'plan'}<span data-testid="mission-plan-mode">Plan, tracked here and not run by fleet</span>{:else}{#if mission.mode === 'continuous'}Continuous{#if wakeLabel(mission.policy?.wake_every_secs)}, wakes {wakeLabel(mission.policy?.wake_every_secs)}{/if} · {/if}L{mission.level} asked
            · {mission.policy?.max_parallel ?? POLICY_DEFAULT_PARALLEL} at once{/if}
          {#if progressLabel(mission)} · {progressLabel(mission)}{/if}
          · updated {timeAgo(mission.updated_at)}
        </p>
        {#if plan}
          <p class="meta" data-testid="mission-autonomy">
            {autonomy?.runs} · {autonomy?.limits} · spent {dollars(plan.cost_micros)}{#if plan.autonomy.grant?.budget_micros}
              of {dollars(plan.autonomy.grant.budget_micros)}{/if} · {plan.counts.open} running
          </p>
        {/if}
        {#if statsText}
          <p class="meta" data-testid="mission-stats" title="Runs per done task, retries, cold starts (sessions started), a task's median time, and the share of the working span with no run open">{statsText}</p>
        {/if}
        {#if mayChange && confirmMove}
          <div class="row confirm-move" role="alertdialog" aria-label="Confirm" data-testid="mission-move-confirm-row">
            <span>{finalMoveQuestion(mission.name, confirmMove)}</span>
            <button
              class="btn"
              class:btn--danger={confirmMove !== 'completed'}
              type="button"
              disabled={busy || changeBlocked}
              data-testid="mission-move-confirm"
              onclick={() => void confirmFinal()}>{moveLabel(mission.state, confirmMove)}</button
            >
            <button class="btn btn--quiet" type="button" data-testid="mission-move-keep" onclick={() => (confirmMove = null)}>Keep</button>
          </div>
        {/if}
        {#if mayChange && ciRed}
          <div class="row confirm-move" role="alertdialog" aria-label="CI is failing" data-testid="mission-ci-override-row">
            <span>CI is failing on this mission's pull requests. Complete it anyway? Fleet records that you did.</span>
            <button
              class="btn btn--danger"
              type="button"
              disabled={busy || changeBlocked}
              data-testid="mission-ci-override"
              onclick={() => void move('completed', true)}>Complete anyway</button
            >
            <button class="btn btn--quiet" type="button" data-testid="mission-ci-keep" onclick={() => (ciRed = false)}>Keep</button>
          </div>
        {/if}
      </header>

      <div class="scroll" bind:this={scroller} onscroll={onScrolled}>
        <div class="col" data-testid="mission-detail-centre">
          {#if notice && !(noticeAt === 'panel' && $missionPanelOpen) && !(noticeAt === 'next' && nextShown)}<p class="notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
          {#if refreshError}
            <p class="refresh-error" role="status" data-testid="mission-refresh-error">
              Couldn't refresh ({readErrorText(refreshError)}) — showing what was loaded.
              <button class="btn btn--quiet" type="button" data-testid="mission-refresh-retry" onclick={() => openedId != null && void loadDetail(openedId)}>Retry</button>
            </p>
          {/if}

          {#if graph}
            <TaskGraphCard {graph} kind="mission" onopen={() => ((tasksView = 'graph'), showMissionPanel('plan'))} />
          {/if}
          {#if liveSessions.length > 0}
            <div class="live" data-testid="mission-room-sessions">
              <span class="muted small">{liveSessions.length} live session{liveSessions.length === 1 ? '' : 's'}</span>
              {#each liveSessions as s (s.session_id)}
                {@const row = $sessionRows.find((r) => r.id === s.session_id)}
                <button
                  type="button"
                  class="sess-chip"
                  class:on={$missionPanelOpen && $missionPanelTab === 'session' && focusRow?.id === s.session_id}
                  title="Open {s.tmux_name} on {s.host_alias} in the panel"
                  data-testid="mission-room-session"
                  onclick={() => openSession(s)}
                  ><StatusDot state={sessionState(row)} size={6} />{titleOf(s.item_id)}
                  · {s.host_alias}</button
                >
              {/each}
            </div>
          {/if}

          <ol class="timeline" aria-label="The mission's log" data-testid="mission-timeline">
            <li class="brief" data-testid="mission-brief">
              <span class="when">{shortAge(mission.created_at)} · created</span>
              <p class="goal">{mission.goal}</p>
            </li>
            {#if events.length > shownEvents.length}
              <li>
                <button class="btn btn--quiet earlier" type="button" data-testid="mission-room-earlier" onclick={() => (earlierShown = true)}
                  >Show {events.length - shownEvents.length} earlier</button
                >
              </li>
            {/if}
            {#each shownEvents as e (e.id)}
              <li class="ev" data-testid="mission-room-event" data-kind={e.kind}>
                <span class="when">{shortAge(e.at)}</span>
                <span>{eventSentence(e)}</span>
              </li>
            {/each}
          </ol>

          {#if limitWait && !plannerFailure}
            <p class="limit-wait" data-testid="mission-limit-wait">{limitWaitSentence(limitWait)}</p>
          {/if}

          {#if plannerFailure}
            <div class="card planner-error" class:planner-wait={plannerIsWait} role="alert" data-testid="mission-planner-error">
              <p class="title">{plannerFailure.title}</p>
              <p>{plannerFailure.text}</p>
              <div class="row">
                {#if mayChange}
                  <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-planner-retry" onclick={() => void askPlanner()}
                    >Retry</button
                  >
                {/if}
                <button
                  class="btn btn--quiet"
                  type="button"
                  aria-expanded={plannerDetails}
                  data-testid="mission-planner-details"
                  onclick={() => (plannerDetails = !plannerDetails)}>Details</button
                >
                <button class="btn btn--quiet" type="button" aria-label="Dismiss" data-testid="mission-planner-dismiss" onclick={() => (plannerFailure = null)}
                  >✕</button
                >
              </div>
              {#if plannerDetails}<pre class="details" data-testid="mission-planner-details-text">{plannerFailure.details}</pre>{/if}
            </div>
          {/if}

          {#if mayChange && proposals.ids.length > 0}
            <div class="card proposals" data-testid="mission-proposals">
              <p class="card-head">
                <span class="tag">Proposed</span>
                <span>{proposalsLine}</span>
              </p>
              {#if reviewId != null}
                <div class="review" data-testid="mission-review">
                  <span class="muted small">Task {reviewAt + 1} of {proposals.ids.length}</span>
                  <span class="review-title">{titleOf(reviewId)}</span>
                  <div class="row">
                    <button class="btn btn--chip" type="button" disabled={busy} data-testid="mission-review-accept" onclick={() => void reviewOne(true)}
                      >Accept</button
                    >
                    <button class="btn btn--quiet" type="button" disabled={busy} data-testid="mission-review-reject" onclick={() => void reviewOne(false)}
                      >Reject</button
                    >
                    <button class="btn btn--quiet" type="button" disabled={busy} data-testid="mission-review-skip" onclick={reviewSkip}>Skip</button>
                    <button class="btn btn--quiet" type="button" data-testid="mission-review-stop" onclick={() => (reviewing = false)}>Done</button>
                  </div>
                </div>
              {:else}
                <p class="titles">
                  {proposals.titles.slice(0, 4).join(' · ')}{#if proposals.titles.length > 4}{' '}· +{proposals.titles.length - 4} more{/if}
                </p>
                <div class="row">
                  <button class="btn btn--chip" type="button" disabled={busy} data-testid="mission-accept-all" onclick={() => void acceptAll()}
                    >Accept all{proposals.ids.length > 1 ? ` ${proposals.ids.length}` : ''}</button
                  >
                  <button
                    class="btn btn--quiet"
                    type="button"
                    disabled={busy}
                    data-testid="mission-review-one"
                    onclick={() => ((reviewing = true), (reviewAt = 0))}>Review one by one</button
                  >
                  <button
                    class="btn btn--quiet"
                    type="button"
                    disabled={busy || changeBlocked}
                    data-testid="mission-proposals-replan"
                    onclick={() => void askPlanner()}>Ask the planner again</button
                  >
                </div>
              {/if}
            </div>
          {/if}
          {#if lastAccepted.length > 0}
            <div class="row line" role="status" data-testid="mission-accepted">
              <span>Accepted {lastAccepted.length}.</span>
              <button class="btn btn--quiet" type="button" disabled={busy} data-testid="mission-undo-accept" onclick={() => void undoAccept()}>Undo</button>
            </div>
          {/if}

          {#if openCards.length > 0}
            <ul class="cards" aria-label="Waiting for you" data-testid="mission-cards">
              {#each openCards as c, i (c.id)}
                <li class="card needs" data-testid="mission-card" data-kind={c.kind}>
                  {#if c.kind === 'ask' && detail.may_change}
                    {@const options = askOptions(c)}
                    <QuestionCard
                      question={cardLine(c)}
                      age={`${c.source === 'planner' ? 'Planner' : 'Fleet'} · asked ${shortAge(c.created_at)}`}
                      label="Question from the mission"
                      testid="mission-card-question"
                      answers={[
                        ...options.map((o, k) => ({
                          label: o,
                          testid: 'mission-card-option',
                          disabled: busy || changeBlocked,
                          onselect: () => void answerWith(c, o),
                          kbd: String(k + 1),
                        })),
                        { label: 'Skip', testid: 'mission-card-skip', disabled: busy || changeBlocked, onselect: () => void decide(c, false) },
                      ]}
                      onownwords={() => (ownWords = { ...ownWords, [c.id]: true })}
                    >
                      {#if ownWords[c.id] || options.length === 0}
                        <span class="row">
                          <input placeholder="Your answer" aria-label="Your answer" bind:value={answers[c.id]} data-testid="mission-card-answer" />
                          <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} data-testid="mission-card-apply" onclick={() => void decide(c, true)}
                            >Answer</button
                          >
                        </span>
                      {/if}
                    </QuestionCard>
                  {:else}
                    <p class="card-head">
                      <StatusChip state="waiting" />
                      <span class="muted small"
                        >{c.source === 'planner' ? 'Planner' : 'Fleet'}{#if openCards.length > 1} · card {i + 1} of {openCards.length}{/if}</span
                      >
                    </p>
                    <p class="card-text">{cardLine(c)}</p>
                    {#if detail.may_change}
                      <div class="row">
                        <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} data-testid="mission-card-apply" onclick={() => void decide(c, true)}
                          >Apply</button
                        >
                        <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-card-dismiss" onclick={() => void decide(c, false)}
                          >Dismiss</button
                        >
                      </div>
                    {:else}
                      <!-- Not this person's to answer: say so, and let them
                           put it away rather than leave it hanging. -->
                      <div class="row">
                        <span class="muted small">Only someone who may change this mission can answer.</span>
                        <button class="btn btn--quiet" type="button" data-testid="mission-card-hide" onclick={() => (hiddenCards = [...hiddenCards, c.id])}
                          >Hide</button
                        >
                      </div>
                    {/if}
                  {/if}
                  {#if cardNote?.id === c.id}<p class="notice" role="status" data-testid="mission-card-notice">{cardNote.text}</p>{/if}
                </li>
              {/each}
            </ul>
          {/if}

          {#if !isFinal(mission.state) && mission.state !== 'draft'}
            <!-- Redesign 9.10: a stuck mission's card; Jev proposes, a person picks. -->
            <MissionTriage missionId={mission.id} reload={detail.events?.[0]?.id ?? 0} onstep={triageStep} />
          {/if}

          {#if mayChange && nothingYet}
            <!-- A mission with no tasks had nothing to press in the centre:
                 the ways to get tasks sat in the panel's Plan tab only. -->
            <div class="card" data-testid="mission-empty-next">
              <p class="card-head">
                <span class="tag">Next</span>
                <span>No tasks yet</span>
              </p>
              <p class="muted small">
                Tasks come from the planner, a plan you import, or your own hand in the Plan tab. The mission runs once it has tasks{#if (mission.repos ?? []).length === 0}
                  {' '}and a repo (the Repos tab){/if}.
              </p>
              <div class="row">
                {#if mission.mode !== 'plan'}
                  <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} data-testid="mission-empty-plan" onclick={() => void askPlanner()}
                    >Ask the planner</button
                  >
                {/if}
                <button
                  class="btn btn--quiet"
                  type="button"
                  disabled={busy || importBlocked}
                  data-testid="mission-empty-import"
                  onclick={() => (showMissionPanel('plan'), (importOpen = true))}>Import a plan…</button
                >
                <button class="btn btn--quiet" type="button" data-testid="mission-empty-tasks" onclick={() => showMissionPanel('plan')}>Add tasks by hand</button>
              </div>
            </div>
          {/if}

          {#if nextShown}
            <div class="next" data-testid="mission-next">
              {#if (plan?.steps ?? []).length > 0}
                <ul class="steps" aria-label="Next steps" data-testid="mission-steps">
                  {#each plan?.steps ?? [] as st (stepKey(st))}
                    <li data-testid="mission-step" data-kind={st.kind}>
                      <span class="arrow" aria-hidden="true">→</span>
                      <span>{stepLine(st)}</span>
                      {#if mayChange && mission.state === 'active' && st.kind !== 'ask'}
                        <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} onclick={() => void startWave(stepKey(st))}>Go</button>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if mayChange && mission.state === 'active' && pressable.length > 0}
                <p class="next-line">
                  <span>Next:</span>
                  <!-- Redesign 1.5: the mission's one primary (the edit form's
                       Save takes over while it is open). -->
                  <button
                    class="btn"
                    class:btn--primary={!editing}
                    type="button"
                    disabled={busy || changeBlocked}
                    data-testid="mission-start-wave"
                    onclick={() => void startWave()}>Start wave ({pressable.length})</button
                  >
                  {#if openCards.length > 0}<span class="muted small">once the cards are answered</span>{/if}
                  {#if plan && grantWouldHelp(plan.autonomy)}
                    <!-- Without a grant a person presses every wave; the way
                         to change that sat in the panel's Autonomy block only. -->
                    <button
                      class="link"
                      type="button"
                      data-testid="mission-run-itself"
                      onclick={() => (showMissionPanel('plan'), (granting = true))}>or let it run by itself…</button
                    >
                  {/if}
                </p>
              {/if}
              {#if notice && noticeAt === 'next'}<p class="notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
            </div>
          {/if}

          {#if isFinal(mission.state)}
            <!-- Gap G3.7: the Finish board. Reopen moves it back to paused. -->
            <MissionFinish
              {detail}
              completed={missions.filter((m) => isFinal(m.state)).length}
              mayChange={!!detail.may_change}
              reopenBlocked={changeBlocked}
              nameOf={personName}
              onreopen={() => move('paused')}
              onarchived={load}
            />
          {/if}
          {#if detail.may_change && mission.state === 'completed'}
            <!-- Redesign 9.11: Finish's release note, drafted on demand. -->
            <section class="card" aria-label="Release note">
              <p class="card-head">Release note</p>
              <ReleaseNote missionId={mission.id} />
            </section>
          {/if}

          {#each myReceipts as r (r.key)}
            <p class="receipt" data-testid="mission-steer-receipt">
              <span class="quote">“{r.text}”</span>
              <span class="muted small">Sent to Control · {receiptLabel(r.key, r.operatorId)}</span>
              <button class="link" type="button" data-testid="mission-steer-open" onclick={() => openControl('chat')}>Open in Control ›</button>
            </p>
          {/each}
        </div>
      </div>

      <form class="composer" data-testid="mission-steer" onsubmit={(e) => (e.preventDefault(), steer())}>
        <div class="box">
          <textarea
            rows="2"
            placeholder="Steer {mission.name}: ask, change the plan, retry a step…"
            aria-label="Steer {mission.name}"
            title="Goes to Control as: {steerPrefix(mission)} …"
            disabled={!!blocked}
            bind:value={steerText}
            onkeydown={steerKey}
            data-testid="mission-steer-input"
          ></textarea>
          <button class="btn send" type="submit" disabled={!!blocked || !!steerWait || !steerText.trim()} aria-label="Send to Control" data-testid="mission-steer-send"
            >↵</button
          >
        </div>
        {#if blocked}
          <p class="muted small" data-testid="mission-steer-blocked">
            {blocked}
            <button class="link" type="button" onclick={() => openControl('chat')}>Open Control</button>
          </p>
        {:else if steerWait}
          <p class="muted small" data-testid="mission-steer-busy">
            {steerWait}
            {#if $parkedHeld.length > 0}<button class="link" type="button" onclick={() => openControl('chat')}>Open Control</button>{/if}
          </p>
        {:else}
          <p class="muted small hint">Goes to Control, about this mission.</p>
        {/if}
      </form>
    {:else if $roomMissionId != null}
      <div class="scroll">
        <div class="col">
          {#if notice}<p class="notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
          <div data-testid="mission-room-loading"><Skeleton rows={4} label="Reading the mission" /></div>
        </div>
      </div>
    {:else}
      <div class="starter" data-testid="mission-room-starter">
        {#if notice}<p class="notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
        <OrbitMark size={56} label={null} />
        <h2 class="starter-title">{missions.length === 0 ? 'No missions yet' : 'Pick a mission'}</h2>
        <p class="muted">
          {#if missions.length === 0}
            A mission is a goal with the tasks that reach it. Fleet runs them wave by wave and asks you where it matters.
          {:else}
            {missions.filter((m) => m.state === 'active').length} running · {missions.filter((m) => m.waiting_on).length} need you. Pick one on the left, or start another.
          {/if}
        </p>
        <div class="starters">
          <button class="btn btn--primary" type="button" disabled={saveBlocked} data-testid="mission-room-new" onclick={startNewMission}
            >New mission</button
          >
          <button
            class="btn"
            type="button"
            title="Control's /plan turns a task or an idea into steps, sessions and a graph"
            data-testid="mission-room-plan-task"
            onclick={() => void askAgent('/plan ')}>Plan from a task</button
          >
        </div>
      </div>
    {/if}
  </div>

  {#if showing && $missionPanelOpen && mission && detail}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="right-handle" onpointerup={endRight} onpointercancel={endRight}>
      <Resizer id="missions-right" onresize={onResizeRight} />
    </div>
    <aside class="panel" aria-label="Mission panel" data-testid="mission-panel">
      <div class="strip">
        <div class="tabs" role="tablist" aria-label="Mission" use:tablistKeys data-testid="mission-tabs">
          {#each MISSION_PANEL_TABS as t (t.id)}
            {@const n = tabCount(t.id)}
            <button
              type="button"
              role="tab"
              class="tab"
              aria-selected={$missionPanelTab === t.id}
              data-testid="mission-tab-{t.id}"
              onclick={() => missionPanelTab.set(t.id)}>{t.label}{#if n}<span class="count">{n}</span>{/if}</button
            >
          {/each}
        </div>
        <span class="grow"></span>
        <button type="button" class="icon" title="Hide the panel" aria-label="Hide the panel" data-testid="mission-panel-close" onclick={() => missionPanelOpen.set(false)}
          >›</button
        >
      </div>
      <div class="body" role="tabpanel" aria-label={MISSION_PANEL_TABS.find((t) => t.id === $missionPanelTab)?.label}>
        {#if notice && noticeAt === 'panel'}<p class="notice panel-notice" role="status" data-testid="mission-notice">{notice}</p>{/if}
        {#if $missionPanelTab === 'session'}
          {#if focusRow}
            <SessionFocus
              session={focusRow}
              siblings={liveRows.length > 1 ? liveRows : []}
              siblingsTitle="In this mission · {liveRows.length} live sessions"
              onpick={(row) => openMissionSession(row.id)}
              onfull={() => openFull(focusRow)}
              prefix="mission-session"
            >
              {#snippet back()}
                {#if focusLink}
                  <p class="on-task muted small">
                    {focusLink.kind} on
                    <button class="link" type="button" title="Open the task" data-testid="mission-session-task" onclick={() => openItem(focusLink.item_id)}
                      >{titleOf(focusLink.item_id)}</button
                    >
                  </p>
                {/if}
              {/snippet}
            </SessionFocus>
          {:else}
            <p class="empty" data-testid="mission-sessions-empty">
              {liveSessions.length > 0 ? "This mission's sessions are no longer in the fleet." : "No session is working on this mission's tasks."}
            </p>
          {/if}
        {:else if $missionPanelTab === 'plan'}
          <div class="pane">
            {#if editing}
              <div class="edit" data-testid="mission-edit-form">
                <label class="field">Goal<textarea rows="3" bind:value={editGoal} data-testid="mission-edit-goal"></textarea></label>
                <label class="field">Not in scope<textarea rows="2" bind:value={editNonGoals}></textarea></label>
                <label class="field"
                  >Done when (one condition per line)<textarea rows="3" bind:value={editDoneWhen} data-testid="mission-edit-done-when"></textarea></label
                >
                <label class="field"
                  >Autonomy
                  <select bind:value={editLevel} data-testid="mission-edit-level">
                    {#each MISSION_LEVELS as l (l.level)}<option value={l.level}>{l.label}</option>{/each}
                  </select></label
                >
                <div class="row">
                  <label class="field inline"
                    >Mode
                    <select bind:value={editMode}>
                      <option value="finite">Finite</option>
                      <option value="continuous">Continuous</option>
                      <option value="plan">Plan (tracked, not run)</option>
                    </select></label
                  >
                  <label class="field inline"
                    >Parallel runs
                    <input type="number" min="1" max={POLICY_MAX_PARALLEL} bind:value={editParallel} class="role" data-testid="mission-edit-parallel" /></label
                  >
                  <label class="field inline" title="By lane: a lane's tasks run one after another in one session on the lane's branch. Changes only while the mission is not running."
                    >Tasks run
                    <select bind:value={editExecution} disabled={mission.state === 'active'} data-testid="mission-edit-execution">
                      <option value="item">Each in its own session</option>
                      <option value="lane">By lane, one session each</option>
                    </select></label
                  >
                  {#if editExecution === 'lane'}
                    <label
                      class="field inline"
                      title="At the lane's end: a task waiting for its test does not hold the lane, and one test run in the lane's checkout answers all its tasks. Changes only while the mission is not running."
                      >Tests run
                      <select bind:value={editLaneTestAt} disabled={mission.state === 'active'} data-testid="mission-edit-lane-test-at">
                        <option value="item">After each task</option>
                        <option value="lane">At the lane's end</option>
                      </select></label
                    >
                    <label
                      class="field inline"
                      title="Fleet opens: once a task of a lane is done, fleet pushes the lane's branch up to it and opens its draft pull request; ready and merge stay yours."
                      >Lane PR
                      <select bind:value={editLanePr} data-testid="mission-edit-lane-pr">
                        <option value="off">The lead opens it</option>
                        <option value="draft">Fleet opens a draft</option>
                      </select></label
                    >
                  {/if}
                  {#if editMode === 'continuous'}
                    <label class="field inline"
                      >Wake every (min)
                      <input
                        type="number"
                        min={minWakeMins}
                        placeholder="no timer"
                        bind:value={editWakeMins}
                        class="role"
                        data-testid="mission-edit-wake"
                      /></label
                    >
                  {/if}
                </div>
                {#if wakeBad}<p class="muted small" role="alert" data-testid="mission-edit-wake-bad">A continuous mission wakes at most every {minWakeMins} minutes.</p>{/if}
                {#if parallelBad}<p class="muted small" role="alert">Parallel runs is 1 to {POLICY_MAX_PARALLEL}.</p>{/if}
                <div class="row">
                  <button
                    class="btn btn--primary"
                    type="button"
                    disabled={busy || saveBlocked || wakeBad || parallelBad}
                    data-testid="mission-edit-save"
                    onclick={() => void saveEdit()}>Save</button
                  >
                  <button class="btn btn--quiet" type="button" onclick={() => (editing = false)}>Cancel</button>
                </div>
              </div>
            {:else}
              <section class="sec-block" aria-label="Goal">
                <h3>Goal</h3>
                <p class="goal">{mission.goal}</p>
                {#if (mission.done_when ?? []).length > 0}
                  <h3>Done when</h3>
                  <ul class="done-when" aria-label="Done when">
                    {#each mission.done_when ?? [] as c, i (i)}<li>○ {c}</li>{/each}
                  </ul>
                {/if}
                {#if mission.non_goals}
                  <h3>Not in scope</h3>
                  <p class="goal">{mission.non_goals}</p>
                {/if}
              </section>
            {/if}

            <section class="sec-block" aria-label="Autonomy" data-testid="mission-loop">
              <h3>Autonomy</h3>
              {#if autonomy?.hint}<p class="muted small" data-testid="mission-autonomy-hint">{autonomy.hint}</p>{/if}
              <dl class="kv" data-testid="mission-facts">
                {#if owner}<dt>Owner</dt><dd data-testid="mission-owner">{owner}</dd>{/if}
                {#if mission.mode !== 'plan'}
                  <dt>Planner</dt><dd data-testid="mission-planner-runs">{plannerLine(detail)}</dd>
                  <dt>Brakes</dt>
                  <dd data-testid="mission-brakes"
                    >{brakesLine(mission.policy, plan?.autonomy.grant?.revoked_at == null ? plan?.autonomy.grant?.budget_micros : null)}</dd
                  >
                {/if}
              </dl>
              {#if mayChange && plan}
                <div class="row">
                  <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-plan" onclick={() => void askPlanner()}
                    >Ask the planner</button
                  >
                  {#if plan.autonomy.grant}
                    <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-revoke" onclick={() => void revokeGrant()}
                      >End the grant (L{plan.autonomy.grant.level}, {plan.autonomy.grant.until_done
                        ? 'until the mission completes'
                        : `until ${untilWords(plan.autonomy.grant.expires_at)}`})</button
                    >
                  {:else}
                    <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-grant" onclick={() => (granting = !granting)}
                      >Grant a budget so it runs by itself…</button
                    >
                  {/if}
                </div>
                {#if granting}
                  <form class="grant" data-testid="mission-grant-form" use:formKeys onsubmit={(e) => (e.preventDefault(), void grant())}>
                    <label class="field inline"
                      >Level
                      <select bind:value={grantLevel}>
                        {#each MISSION_LEVELS.filter((l) => l.level >= 1) as l (l.level)}<option value={l.level}>{l.label}</option>{/each}
                      </select></label
                    >
                    <div class="row">
                      {#if !(grantUntilDone && mission.mode === 'finite')}
                        <label class="field inline">Hours <input type="number" min="1" max="168" bind:value={grantHours} class="role" /></label>
                      {/if}
                      <label class="field inline"
                        >Budget $ <input
                          placeholder={grantUntilDone && mission.mode === 'finite' ? 'required' : 'none'}
                          bind:value={grantBudget}
                          class="role"
                          inputmode="decimal"
                          aria-invalid={grantBudgetBad ? 'true' : undefined}
                          aria-describedby={grantBudgetBad ? 'mission-grant-budget-bad' : undefined}
                          data-testid="mission-grant-budget"
                        /></label
                      >
                    </div>
                    {#if grantBudgetBad}<p class="muted small" id="mission-grant-budget-bad" role="alert" data-testid="mission-grant-budget-bad">{grantBudgetBad}</p>{/if}
                    {#if mission.mode === 'finite'}
                      <label class="field inline"
                        ><input type="checkbox" bind:checked={grantUntilDone} data-testid="mission-grant-until-done" /> Until the mission completes
                        <span class="muted small">(the budget is its only bound)</span></label
                      >
                    {/if}
                    {#if $hosts.length > 0}
                      <fieldset class="hosts" data-testid="mission-grant-hosts">
                        <legend>Hosts <span class="muted small">(none ticked: any)</span></legend>
                        {#each $hosts as h (h.alias)}
                          <label class="host"
                            ><input type="checkbox" value={h.alias} bind:group={grantHosts} data-testid="mission-grant-host" /> {h.alias}</label
                          >
                        {/each}
                      </fieldset>
                    {/if}
                    <div class="row">
                      <button class="btn" type="submit" disabled={busy || !!grantBudgetBad} data-testid="mission-grant-save"
                        >{grantSignLabel(
                          grantLevel,
                          grantUntilDone && mission.mode === 'finite' ? { untilDone: true } : { hours: grantHours },
                          grantBudgetBad ? undefined : microsOf(grantBudget),
                        )}</button
                      >
                      <button class="btn btn--quiet" type="button" data-testid="mission-grant-cancel" onclick={() => (granting = false)}>Cancel</button>
                    </div>
                  </form>
                {/if}
              {/if}
            </section>

            <section class="sec-block" aria-label="Tasks">
              <h3>Tasks</h3>
              <MissionTasks
                {detail}
                {mayChange}
                {busy}
                {changeBlocked}
                act={panelAct}
                bind:tasksView
                onopenitem={openItem}
                onopensession={openSession}
                onnotice={(t) => ((notice = t), (noticeAt = 'panel'))}
              />
            </section>

            {#if mayChange && importOpen}
              <div class="import" data-testid="mission-import">
                <PlanImportForm bind:text={importText} bind:ready={importRows} />
                <span class="row">
                  <button class="btn" type="button" disabled={busy || importBlocked || !importRows} data-testid="mission-import-run" onclick={() => void runImport()}
                    >Import {importRows?.length ?? 0} steps</button
                  >
                  <button class="btn btn--quiet" type="button" onclick={() => ((importOpen = false), (importText = ''))}>Cancel</button>
                </span>
                {#if mission.mode !== 'plan'}
                  <p class="muted small">A {mission.mode} mission holds 30 tasks; make it a Plan to hold up to 200.</p>
                {/if}
              </div>
            {/if}
            {#if importResult}
              <p class="muted small" role="status" data-testid="mission-import-result">{importResult}</p>
            {/if}
            {#if importUnknown.length > 0}
              <p class="muted small" data-testid="mission-import-unknown">Needs that name no step: {importUnknown.join(', ')}</p>
            {/if}

            {#if detail.may_change}
              {#if confirmDelete}
                <div class="row" data-testid="mission-delete-row">
                  <span>Delete {mission.name}? Its tasks stay.</span>
                  <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = false)}>Keep</button>
                  <button class="btn btn--danger" type="button" disabled={busy} data-testid="mission-delete-confirm" onclick={() => void remove()}
                    >Delete</button
                  >
                </div>
              {:else}
                <div class="row foot-actions">
                  {#if mayChange && !importOpen}
                    <button class="btn btn--quiet" type="button" disabled={busy || importBlocked} data-testid="mission-import-open" onclick={() => (importOpen = true)}
                      >Import a plan…</button
                    >
                  {/if}
                  {#if mayChange && split.menu.includes('completed')}
                    <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-finish-open" onclick={() => pickFinal('completed')}
                      >Finish…</button
                    >
                  {/if}
                  {#if mission.state === 'draft' || isFinal(mission.state)}
                    <button class="btn btn--quiet danger" type="button" data-testid="mission-delete" onclick={() => (confirmDelete = true)}>Delete…</button>
                  {/if}
                </div>
              {/if}
            {/if}
          </div>
        {:else if $missionPanelTab === 'runs'}
          <div class="pane">
            {#if runs.length === 0}
              <p class="muted small" data-testid="mission-runs-empty">No runs yet. Start wave starts the first.</p>
            {:else}
              <ul class="runs" aria-label="Runs" data-testid="mission-runs">
                {#each runs as r (r.task_id)}
                  <li data-testid="mission-run" data-state={r.state}>
                    <span class="node-dot"><StatusDot state={toneOf(r.state)} label={runStateLabel(r.state)} /></span>
                    <span class="main">
                      {#if r.item_id != null}
                        {@const itemId = r.item_id}
                        <span class="title-row">
                          <button class="title linkish" type="button" title="Open the task" data-testid="mission-run-open" onclick={() => openItem(itemId)}
                            >{titleOf(itemId)}</button
                          >
                          {#each liveSessions.filter((s) => s.item_id === itemId) as s (s.session_id)}
                            <button
                              class="btn btn--chip"
                              type="button"
                              title="Open {s.tmux_name} on {s.host_alias}"
                              data-testid="mission-task-session"
                              data-session-id={s.session_id}
                              onclick={() => openSession(s)}>Session</button
                            >
                          {/each}
                        </span>
                      {:else}
                        <span class="title">Task run {r.task_id}</span>
                      {/if}
                      <span class="muted small"
                        >{r.role}{r.attempt ? ` #${r.attempt}` : ''} · {runStateLabel(r.state)}{#if r.at != null} · {shortAge(r.at)}{/if}{#if r.actor} · by {r.actor ===
                          'loop'
                            ? 'the loop'
                            : r.actor}{/if}</span
                      >
                    </span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {:else if $missionPanelTab === 'log'}
          <div class="pane">
            <ul class="events" data-testid="mission-events">
              {#each detail.events ?? [] as e (e.id)}
                <li><span class="muted small">{shortAge(e.at)}</span> {eventSentence(e)}</li>
              {/each}
            </ul>
          </div>
        {:else}
          <div class="pane">
            {#if (mission.repos ?? []).length === 0}
              <p class="muted small">No repo allowed yet: a run needs at least one.</p>
            {:else}
              <ul class="repos" data-testid="mission-repos">
                {#each mission.repos ?? [] as r (r.project_id)}
                  <li>
                    <span class="title">{r.name}</span>
                    {#if r.role}<span class="muted small">{r.role}</span>{/if}
                    {#if mayChange}
                      <button
                        class="btn btn--quiet btn--icon"
                        type="button"
                        aria-label="Remove repo"
                        title="Remove repo"
                        disabled={busy}
                        onclick={() => void act(setMissionRepo(mission.id, r.project_id, false), 'panel')}>×</button
                      >
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
            {#if mayChange && repoChoices.length > 0}
              <div class="row">
                <select bind:value={repoPick} aria-label="Add a repo" data-testid="mission-repo-pick">
                  <option value="">Add a repo…</option>
                  {#each repoChoices as p (p.id)}<option value={p.id}>{p.owner}/{p.repo}</option>{/each}
                </select>
                <input class="role" placeholder="role" aria-label="Role" bind:value={repoRole} />
                <button class="btn" type="button" disabled={busy || repoPick === ''} data-testid="mission-repo-add" onclick={() => void addRepo()}>Add</button>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </aside>
  {:else}
    <div></div>
    <div></div>
  {/if}
</section>

<style>
  .room {
    display: grid;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    font-size: var(--text-xs);
  }
  .room > :global(*) {
    min-width: 0;
    min-height: 0;
  }
  .centre {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    max-width: calc(760px + 2 * var(--space-4));
    margin: 0 auto;
    padding: var(--space-3) var(--space-4) var(--space-2);
    box-sizing: border-box;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    flex-wrap: wrap;
  }
  .name {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: 600;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .grow {
    flex: 1 1 auto;
  }
  .meta {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    max-width: 760px;
    margin: 0 auto;
    padding: var(--space-2) var(--space-4) var(--space-4);
  }
  .live {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .sess-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 20px;
    padding: 0 7px;
    border: 0;
    border-radius: var(--radius-sm);
    background: var(--chip-bg);
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }
  .sess-chip.on {
    background: var(--accent-soft);
    color: var(--fg);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ev {
    display: flex;
    gap: var(--space-2);
    color: var(--fg-muted);
    font-size: var(--text-xs);
    line-height: var(--text-xs-lh);
  }
  .when {
    flex: none;
    min-width: 2.5rem;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
  }
  .brief {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: var(--space-2);
  }
  .goal {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--text-sm);
    line-height: var(--text-sm-lh);
    color: var(--fg);
  }
  .earlier {
    align-self: flex-start;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
  }
  .card.needs {
    border-color: var(--status-waiting);
  }
  .card p {
    margin: 0;
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 500;
  }
  .card-text,
  .titles {
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .titles {
    color: var(--fg-2);
    font-size: var(--text-xs);
  }
  .tag {
    font-size: var(--text-2xs);
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    color: var(--fg);
  }
  .review {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .review-title {
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .cards {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .cards input {
    flex: 1 1 8rem;
    min-width: 0;
  }
  .next {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .steps li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
    color: var(--fg-2);
  }
  .arrow {
    color: var(--fg-muted);
  }
  .next-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    margin: 0;
  }
  .receipt {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: flex-end;
    gap: 6px;
    margin: 0;
  }
  .quote {
    max-width: min(520px, 85%);
    padding: 8px 12px;
    border-radius: var(--radius-phone-card) var(--radius-phone-card) var(--radius-sm) var(--radius-phone-card);
    background: var(--bg-raise);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--text-sm);
  }
  .composer {
    width: 100%;
    max-width: calc(760px + 2 * var(--space-4));
    margin: 0 auto;
    padding: var(--space-2) var(--space-4) var(--space-3);
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .box {
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
    padding: 8px 10px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-lg);
    background: var(--bg-pane);
  }
  .box:focus-within {
    border-color: var(--accent);
  }
  .box textarea {
    flex: 1 1 auto;
    min-width: 0;
    border: 0;
    padding: 0;
    resize: none;
    background: transparent;
    font-size: var(--text-sm);
  }
  .send {
    flex: none;
  }
  .hint {
    padding-left: 2px;
  }
  .starter {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-6, 2rem) var(--space-4);
    text-align: center;
  }
  .starter-title {
    margin: var(--space-2) 0 0;
    font-size: var(--text-lg);
    font-weight: 600;
  }
  .starter .muted {
    max-width: 28rem;
  }
  .starters {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
    justify-content: center;
    margin-top: var(--space-2);
  }
  .create {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .create .start {
    align-self: flex-start;
  }
  .more {
    position: relative;
    display: inline-flex;
  }
  .more-menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 30;
    min-width: 11rem;
    padding: 0.25rem;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-pop);
  }
  .mi {
    text-align: left;
    border: none;
    background: transparent;
    color: var(--fg);
    cursor: pointer;
    font: inherit;
    font-size: var(--text-xs);
    padding: 0.35rem 0.5rem;
    border-radius: var(--radius-sm);
  }
  .mi:hover,
  .mi:focus-visible {
    background: var(--bg-hover);
  }
  .mi.danger,
  .danger {
    color: var(--danger);
  }
  .confirm-move {
    margin-top: 0.3rem;
  }
  .row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .line {
    font-size: var(--text-xs);
  }
  input,
  textarea,
  select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .row input {
    flex: 1 1 8rem;
    min-width: 0;
  }
  .row input.role {
    flex: 0 1 6rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .field.inline {
    flex: 1 1 8rem;
  }
  .check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .right-handle {
    display: flex;
  }
  .right-handle > :global(.resizer) {
    flex: 1;
  }
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    border-left: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .strip {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    min-height: var(--header-h);
    padding: 0 var(--space-2);
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    gap: 2px;
    min-width: 0;
    overflow-x: auto;
    align-self: stretch;
    align-items: flex-end;
  }
  .tab,
  .icon {
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }
  .tab {
    border-bottom: 2px solid transparent;
    padding: 10px 2px 8px;
    margin: 0 6px;
    font-size: var(--text-xs);
  }
  .tab[aria-selected='true'] {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .icon {
    font-size: var(--text-sm);
    padding: 4px 6px;
    border-radius: var(--radius-md);
  }
  .icon:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }
  .tab:focus-visible,
  .icon:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .count {
    font-size: var(--text-2xs);
    font-weight: 500;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--count-bg);
    color: var(--fg-2);
  }
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .pane {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-3);
  }
  .sec-block {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .edit,
  .grant {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  h3 {
    margin: 0.3rem 0 0;
    font-size: var(--text-2xs);
    font-weight: 500;
    color: var(--fg-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .done-when li {
    padding: 0.1rem 0;
  }
  .kv {
    display: grid;
    grid-template-columns: 4.5rem minmax(0, 1fr);
    gap: 0.2rem 0.6rem;
    margin: 0.2rem 0 0;
    font-size: var(--text-2xs);
  }
  .kv dt {
    color: var(--fg-muted);
  }
  .kv dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .hosts {
    border: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
  }
  .hosts legend {
    padding: 0;
    margin-right: var(--space-1);
    float: left;
  }
  .import {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .foot-actions {
    border-top: 1px solid var(--border);
    padding-top: var(--space-2);
  }
  .runs li,
  .repos li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--border);
  }
  .events li {
    padding: 0.1rem 0;
  }
  .node-dot {
    display: inline-flex;
    align-items: center;
    height: 1lh;
    flex: 0 0 auto;
  }
  .main {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }
  .title {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .linkish {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .linkish:hover,
  .linkish:focus-visible {
    color: var(--accent);
    text-decoration: underline;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .on-task {
    margin: 0 0 var(--space-1);
  }
  .empty {
    padding: var(--space-3);
    margin: 0;
    color: var(--fg-muted);
  }
  .muted {
    color: var(--fg-muted);
    margin: 0;
  }
  .small {
    font-size: var(--text-2xs);
  }
  .notice {
    margin: 0;
    color: var(--danger);
  }
  .panel-notice {
    padding: 8px 12px 0;
  }
  .refresh-error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--usage-warn);
  }
  .planner-error {
    border-color: var(--danger);
  }
  .planner-error .title {
    color: var(--danger);
    font-weight: 600;
  }
  .planner-wait {
    border-color: var(--status-waiting);
  }
  .planner-wait .title {
    color: var(--status-waiting);
  }
  .limit-wait {
    margin: var(--space-1) 0 0;
    font-size: var(--text-xs);
    color: var(--status-waiting);
  }
  .planner-error .details {
    margin: var(--space-1) 0 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
</style>
