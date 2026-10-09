<script lang="ts">
  import Skeleton from './states/Skeleton.svelte';
  import Icon from './kit/Icon.svelte';
  import StatusDot from './kit/StatusDot.svelte';
  import { tablistKeys } from './tablist_keys';
  // Missions (orchestration O1, design 2026-10-07 §9): the Work view's third
  // tab. A mission is a goal over a root task: its member tasks, the repos
  // it may run in, its lifecycle and its log, and its loop (O4–O8): the
  // next steps a person presses (Start wave), the confirm queue of the
  // planner's cards, and the grant that lets the loop act by itself.
  //
  // Text a person or a tracker wrote (names, goals, task titles) is rendered
  // as text, never as markup.
  import { onDestroy, onMount } from 'svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { projects } from './projects';
  import { readPref, writePref } from './prefs';
  import ReleaseNote from './ReleaseNote.svelte';
  import MissionTriage from './MissionTriage.svelte';
  import type { NextStep } from './mission_triage';
  import MissionGraph from './MissionGraph.svelte';
  import Loader from './Loader.svelte';
  import { defaultLaneBy, toneOf, type LaneBy } from './mission_graph';
  import { PLAN_IMPORT_MAX_ROWS, importLine, importMissionPlan, parsePlan } from './plan_import';
  import { hosts } from './hosts';
  import { shortAge, timeAgo } from './session_status';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import { NEWER_HUB, isOlderHub, readErrorText as rawErrorText } from './work_view';
  import type { IpcError } from './result';
  import {
    finalMoveQuestion,
    splitMoves,
    createMission,
    deleteMission,
    doneWhenRows,
    eventSentence,
    getMission,
    isFinal,
    listMissions,
    moveLabel,
    progressLabel,
    setMissionItem,
    setMissionRepo,
    setMissionState,
    stateLabel,
    updateMission,
    acceptWorkProposals,
    nodeLabel,
    openProposals,
    setWorkDep,
    setWorkHold,
    undoWorkAccept,
    wavesOf,
    attemptLine,
    checkGlyph,
    checkable,
    setWorkDoneWhen,
    verificationLabel,
    verifyWorkItem,
    cardLine,
    decideMissionCard,
    dollars,
    grantMission,
    pauseAllMissions,
    planMission,
    retryWorkItem,
    revokeMissionGrant,
    startMissionWave,
    stepKey,
    stepLine,
    plannerError,
    plannerRefusal,
    withoutConfigKeys,
    autonomyWords,
    policyWith,
    wakeLabel,
    missionOpenRequest,
    POLICY_DEFAULT_PARALLEL,
    POLICY_MAX_PARALLEL,
    POLICY_MIN_WAKE_SECS,
    type HumanError,
    type MissionCard,
    trailNodes,
    type GraphNode,
    type Mission,
    type MissionDetail,
  } from './missions';

  /** An error's text for a notice, with no settings key in it (step 1.3). */
  const readErrorText = (e: IpcError) => withoutConfigKeys(rawErrorText(e));

  let missions = $state.raw<Mission[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
  /** The last planner press that failed, in words (step 1.3). */
  let plannerFailure = $state<HumanError | null>(null);
  let plannerDetails = $state(false);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  let selectedId = $state<number | null>(null);
  let detail = $state.raw<MissionDetail | null>(null);

  let creating = $state(false);
  let newName = $state('');
  let newGoal = $state('');

  let editing = $state(false);
  let editGoal = $state('');
  let editNonGoals = $state('');
  let editDoneWhen = $state('');
  let editLevel = $state(0);
  let editMode = $state('finite');
  /** Runs open at once, and a continuous mission's wake interval in minutes
   *  ('' = no timer) (step 1.8). */
  let editParallel = $state(POLICY_DEFAULT_PARALLEL);
  let editWakeMins = $state<number | null>(null);
  const minWakeMins = POLICY_MIN_WAKE_SECS / 60;
  const wakeBad = $derived(
    editMode === 'continuous' && editWakeMins != null && !(editWakeMins >= minWakeMins),
  );
  const parallelBad = $derived(!(Number.isInteger(editParallel) && editParallel >= 1 && editParallel <= POLICY_MAX_PARALLEL));

  let newTask = $state('');
  let repoPick = $state<number | ''>('');
  let repoRole = $state('');
  let confirmDelete = $state(false);
  // Parity row P19: Complete, Mark failed and Cancel live in a
  // ⋯ menu beside Edit and Pause, and each asks before it ends the mission.
  let moreOpen = $state(false);
  let confirmMove = $state<string | null>(null);
  let moreEl = $state<HTMLElement>();

  const saveBlocked = $derived(!!hubActionBlocked('save_mission', $hubStatus, $hubConnection));
  const changeBlocked = $derived(!!hubActionBlocked('set_mission_state', $hubStatus, $hubConnection));

  const mission = $derived(detail?.mission ?? null);

  // The task graph: List keeps every write, Graph draws
  // lanes × waves with the critical path. The choice outlives the mission;
  // the lanes reset to the mission's own default when another one opens.
  type TasksView = 'list' | 'graph';
  const isTasksView = (v: unknown): v is TasksView => v === 'list' || v === 'graph';
  let tasksView = $state<TasksView>(readPref<TasksView>('work.missions.view', 'list', isTasksView));
  $effect(() => writePref('work.missions.view', tasksView));
  const showGraph = $derived(tasksView === 'graph');
  let laneOverride = $state<{ mission: number; by: LaneBy } | null>(null);
  const laneBy = $derived(
    laneOverride && laneOverride.mission === mission?.id ? laneOverride.by : detail ? defaultLaneBy(detail) : 'none',
  );
  // Import plan: a markdown step table read into the mission's tasks.
  let importOpen = $state(false);
  let importText = $state('');
  let importResult = $state<string | null>(null);
  let importUnknown = $state<string[]>([]);
  const parsed = $derived(importText.trim() ? parsePlan(importText) : null);
  const parsedLanes = $derived(parsed ? new Set(parsed.rows.map((r) => r.lane).filter(Boolean)).size : 0);
  const parsedLinks = $derived(parsed ? parsed.rows.reduce((n, r) => n + (r.needs?.length ?? 0), 0) : 0);
  const importBlocked = $derived(!!hubActionBlocked('import_mission_plan', $hubStatus, $hubConnection));

  async function runImport() {
    if (!mission || !parsed || parsed.rows.length === 0) return;
    const out = await act(importMissionPlan(mission.id, parsed.rows));
    if (out) {
      importResult = importLine(out);
      importUnknown = out.unknown_needs ?? [];
      importText = '';
      importOpen = false;
      tasksView = 'graph';
    }
  }

  function repoName(id: number): string | null {
    const own = (mission?.repos ?? []).find((r) => r.project_id === id);
    if (own) return own.name;
    const p = $projects.find((x) => x.project.id === id)?.project;
    return p ? `${p.owner}/${p.repo}` : null;
  }
  const waves = $derived(detail ? wavesOf(detail) : []);
  const proposals = $derived(detail ? openProposals(detail) : []);
  const itemById = $derived(new Map((detail?.items ?? []).map((i) => [i.id, i])));
  // The last "Accept all", for Undo: the hub takes it back only while
  // nothing has touched those tasks (`store::ACCEPT_UNDO_SECS`).
  let lastAccepted = $state<number[]>([]);

  function titleOf(id: number): string {
    const it = itemById.get(id);
    if (it) return it.title;
    const o = (detail?.graph?.outside ?? []).find((x) => x.id === id);
    return o ? (o.key ?? o.title) : `#${id}`;
  }

  /** The members `n` may start waiting for: not itself, not already. */
  function depChoices(n: GraphNode): { id: number; title: string }[] {
    const already = new Set(n.depends_on ?? []);
    return (detail?.items ?? [])
      .filter((i) => i.id !== n.item_id && !already.has(i.id) && i.proposal_state !== 'rejected')
      .map((i) => ({ id: i.id, title: i.title }));
  }

  async function setDep(itemId: number, dependsOn: number, on: boolean) {
    await act(setWorkDep(itemId, dependsOn, on));
  }

  // The task whose condition lines are being edited, and their text.
  let condsFor = $state<number | null>(null);
  let condText = $state('');

  function editConds(itemId: number) {
    condsFor = itemId;
    condText = (itemById.get(itemId)?.done_when ?? []).join('\n');
  }

  async function saveConds() {
    if (condsFor == null) return;
    if (await act(setWorkDoneWhen(condsFor, doneWhenRows(condText)))) condsFor = null;
  }

  async function check(itemId: number, line: string, ok: boolean) {
    await act(verifyWorkItem(itemId, line, ok));
  }

  async function setHold(itemId: number, on: boolean) {
    await act(setWorkHold(itemId, on));
  }

  async function acceptAll() {
    const ids = proposals;
    if (await act(acceptWorkProposals(ids))) lastAccepted = ids;
  }

  async function undoAccept() {
    const ids = lastAccepted;
    if (await act(undoWorkAccept(ids))) lastAccepted = [];
  }
  // The loop (orchestration O4–O6).
  const plan = $derived(detail?.plan ?? null);
  /** The header's autonomy in words: "Runs at L1 · L3 asked · L1 ceiling". */
  const autonomy = $derived(plan ? autonomyWords(plan.autonomy) : null);
  const pressable = $derived((plan?.steps ?? []).filter((s) => s.kind !== 'ask'));
  const openCards = $derived((plan?.cards ?? []).filter((c) => c.state === 'open'));
  // Comet trails beside the mission's current steps (redesign step 9.12):
  // only while it runs, never while it waits on a person.
  const trails = $derived(detail ? trailNodes(detail) : new Set<number>());
  let answers = $state<Record<number, string>>({});
  let granting = $state(false);
  let grantLevel = $state(2);
  let grantHours = $state(8);
  let grantBudget = $state('');
  /** Hosts the grant lets the loop run on; none ticked: any host. */
  let grantHosts = $state<string[]>([]);

  /** A short report of what a press did, failures first. */
  function reportSteps(results: { ok: boolean; detail: string }[] | undefined) {
    const bad = (results ?? []).filter((r) => !r.ok);
    if (bad.length > 0) notice = bad.map((r) => r.detail).join(' · ');
    else if ((results ?? []).length === 0) notice = 'Nothing to start right now.';
  }

  async function startWave(step?: string) {
    if (!mission) return;
    const out = await act(startMissionWave(mission.id, step));
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
      document.querySelector('[data-testid="mission-loop"]')?.scrollIntoView({ block: 'nearest' });
    }
  }

  async function askPlanner() {
    if (!mission) return;
    plannerFailure = null;
    plannerDetails = false;
    busy = true;
    notice = null;
    try {
      const r = await planMission(mission.id);
      if (!r.ok) {
        plannerFailure = isOlderHub(r.error) ? { title: "The planner couldn't run", text: NEWER_HUB, details: `${r.error.code} · ${r.error.message}` } : plannerError(r.error);
        if (r.error.code === 'E_CONFLICT') await load();
        return;
      }
      if (r.value.refused) plannerFailure = plannerRefusal(r.value.refused);
      await load();
    } finally {
      busy = false;
    }
  }

  async function decide(c: MissionCard, ok: boolean) {
    const note = c.kind === 'ask' && ok ? (answers[c.id] ?? '').trim() : undefined;
    if (c.kind === 'ask' && ok && !note) {
      notice = 'Write an answer first.';
      return;
    }
    const out = await act(decideMissionCard(c.id, ok, note));
    if (out?.state === 'refused') notice = out.note ?? 'Refused';
  }

  async function grant() {
    if (!mission) return;
    const cents = Math.round(Number(grantBudget) * 100);
    const out = await act(
      grantMission(mission.id, {
        level: grantLevel,
        hours: grantHours,
        ...(grantBudget.trim() && cents > 0 ? { budget_cents: cents } : {}),
        ...(grantHosts.length > 0 ? { hosts: [...grantHosts] } : {}),
      }),
    );
    if (out) {
      granting = false;
      grantHosts = [];
    }
  }

  async function revokeGrant() {
    if (mission) await act(revokeMissionGrant(mission.id));
  }

  async function pauseAll() {
    const out = await act(pauseAllMissions());
    if (out) notice = out.length ? `Paused ${out.length} mission${out.length === 1 ? '' : 's'}.` : 'No active mission to pause.';
  }
  const mayChange = $derived(!!detail?.may_change && !!mission && !isFinal(mission.state));
  const split = $derived(mission ? splitMoves(mission.state) : { inline: [], menu: [] });

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
  const repoChoices = $derived(
    $projects
      .map((p) => p.project)
      .filter((p) => !(mission?.repos ?? []).some((r) => r.project_id === p.id)),
  );

  async function load() {
    const r = await listMissions();
    loaded = true;
    if (r.ok) {
      missions = Array.isArray(r.value) ? r.value : [];
      error = null;
      if (selectedId != null && !missions.some((m) => m.id === selectedId)) {
        selectedId = null;
        detail = null;
      }
    } else {
      error = readErrorText(r.error);
    }
    if (selectedId != null) await loadDetail(selectedId);
  }

  // Bumped per request and by Back: a late answer for a mission no longer
  // open (Back pressed, another opened, a newer load) is dropped (review r07).
  let detailSeq = 0;

  async function loadDetail(id: number) {
    const mine = ++detailSeq;
    const r = await getMission(id);
    if (mine !== detailSeq || selectedId !== id) return;
    if (r.ok) {
      detail = r.value;
    } else {
      notice = readErrorText(r.error);
      detail = null;
      selectedId = null;
    }
  }

  async function open(id: number) {
    lastAccepted = [];
    selectedId = id;
    editing = false;
    confirmDelete = false;
    moreOpen = false;
    confirmMove = null;
    notice = null;
    plannerFailure = null;
    importOpen = false;
    importText = '';
    importResult = null;
    importUnknown = [];
    await loadDetail(id);
  }

  function back() {
    detailSeq++;
    selectedId = null;
    detail = null;
    editing = false;
    notice = null;
    plannerFailure = null;
  }

  /** Run one write; on success re-read the list and the open mission. */
  async function act<T>(p: Promise<import('./result').Result<T>>): Promise<T | null> {
    busy = true;
    notice = null;
    try {
      const r = await p;
      if (!r.ok) {
        notice = readErrorText(r.error);
        if (r.error.code === 'E_CONFLICT') await load();
        return null;
      }
      await load();
      return r.value;
    } finally {
      busy = false;
    }
  }

  async function create() {
    const name = newName.trim();
    const goal = newGoal.trim();
    if (!name || !goal) {
      notice = 'A mission needs a name and a goal.';
      return;
    }
    const m = await act(createMission({ name, goal }));
    if (m) {
      creating = false;
      newName = '';
      newGoal = '';
      await open(m.id);
    }
  }

  function startEdit() {
    if (!mission) return;
    editGoal = mission.goal;
    editNonGoals = mission.non_goals ?? '';
    editDoneWhen = (mission.done_when ?? []).join('\n');
    editLevel = mission.level;
    editMode = mission.mode;
    editParallel = mission.policy?.max_parallel ?? POLICY_DEFAULT_PARALLEL;
    const wake = mission.policy?.wake_every_secs;
    editWakeMins = wake ? Math.round(wake / 60) : null;
    editing = true;
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
          policy: policyWith(mission.policy, { max_parallel: editParallel, wake_every_secs: wakeSecs }),
        },
        mission.version,
      ),
    );
    if (m) editing = false;
  }

  async function move(to: string) {
    if (!mission) return;
    await act(setMissionState(mission.id, to, mission.version));
  }

  async function addTask() {
    if (!mission?.root_item_id) return;
    const title = newTask.trim();
    if (!title) return;
    busy = true;
    const t = await createWorkTask({ title, parent: `item:${mission.root_item_id}` });
    busy = false;
    if (!t.ok) {
      notice = readErrorText(t.error);
      return;
    }
    if (await act(setMissionItem(mission.id, t.value.id, true))) newTask = '';
  }

  async function removeItem(itemId: number) {
    if (!mission) return;
    await act(setMissionItem(mission.id, itemId, false));
  }

  async function addRepo() {
    if (!mission || repoPick === '') return;
    if (await act(setMissionRepo(mission.id, repoPick, true, repoRole))) {
      repoPick = '';
      repoRole = '';
    }
  }

  async function removeRepo(projectId: number) {
    if (!mission) return;
    await act(setMissionRepo(mission.id, projectId, false));
  }

  async function remove() {
    if (!mission) return;
    if (await act(deleteMission(mission.id))) back();
  }

  // A "Sent to a mission" chip in Control asked for this one (redesign 9.3).
  $effect(() => {
    const r = $missionOpenRequest;
    if (!r) return;
    missionOpenRequest.set(null);
    void open(r.id);
  });

  let stopWatching: (() => void) | null = null;
  onMount(() => {
    void load();
    stopWatching = onWorkChangedDebounced(() => void load(), () => 500, () => 3000);
  });
  onDestroy(() => stopWatching?.());
</script>

<div class="missions" data-testid="work-missions">
  {#if notice}
    <p class="notice" role="status" data-testid="mission-notice">{notice}</p>
  {/if}

  {#if mission && detail}
    <div class="detail" data-testid="mission-detail">
      <div class="head">
        <button class="btn btn--quiet" type="button" data-testid="mission-back" onclick={back}>‹ Missions</button>
        <span class="badge" data-testid="mission-state">{stateLabel(mission.state, detail.phase)}</span>
      </div>
      <h3 class="name">{mission.name}</h3>
      <p class="meta muted">
        {#if mission.mode === 'plan'}<span data-testid="mission-plan-mode">Plan, tracked here and not run by fleet</span>{:else}{#if mission.mode === 'continuous'}Continuous{#if wakeLabel(mission.policy?.wake_every_secs)}, wakes {wakeLabel(mission.policy?.wake_every_secs)}{/if} · {/if}L{mission.level} asked
        · {mission.policy?.max_parallel ?? POLICY_DEFAULT_PARALLEL} at once{/if}
        {#if progressLabel(mission)} · {progressLabel(mission)}{/if}
        · updated {timeAgo(mission.updated_at)}
      </p>

      {#if !isFinal(mission.state) && mission.state !== 'draft'}
        <!-- Redesign 9.10: a stuck mission's card; Jev proposes, a person picks. -->
        <MissionTriage missionId={mission.id} reload={detail.events?.[0]?.id ?? 0} onstep={triageStep} />
      {/if}

      {#if editing}
        <label class="field">Goal<textarea rows="3" bind:value={editGoal} data-testid="mission-edit-goal"></textarea></label>
        <label class="field">Not in scope<textarea rows="2" bind:value={editNonGoals}></textarea></label>
        <label class="field"
          >Done when (one condition per line)<textarea rows="3" bind:value={editDoneWhen} data-testid="mission-edit-done-when"
          ></textarea></label
        >
        <div class="row">
          <label class="field inline"
            >Autonomy
            <select bind:value={editLevel} data-testid="mission-edit-level">
              <option value={0}>L0 · asks for everything</option>
              <option value={1}>L1 · You press Start</option>
              <option value={2}>L2 · runs within a grant</option>
              <option value={3}>L3 · reviews and integrates</option>
            </select></label
          >
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
            <input
              type="number"
              min="1"
              max={POLICY_MAX_PARALLEL}
              bind:value={editParallel}
              class="role"
              data-testid="mission-edit-parallel"
            /></label
          >
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
          <button class="btn btn--primary" type="button" disabled={busy || saveBlocked || wakeBad || parallelBad} data-testid="mission-edit-save" onclick={() => void saveEdit()}
            >Save</button
          >
          <button class="btn btn--quiet" type="button" onclick={() => (editing = false)}>Cancel</button>
        </div>
      {:else}
        <p class="goal">{mission.goal}</p>
        {#if mission.non_goals}<p class="muted">Not in scope: {mission.non_goals}</p>{/if}
        {#if (mission.done_when ?? []).length > 0}
          <ul class="done-when" aria-label="Done when">
            {#each mission.done_when ?? [] as c, i (i)}<li>☐ {c}</li>{/each}
          </ul>
        {/if}
        {#if mayChange}
          <div class="row">
            <button class="btn btn--quiet" type="button" disabled={busy || saveBlocked} data-testid="mission-edit" onclick={startEdit}
              >Edit</button
            >
              {#each split.inline as to (to)}
                <button
                  class="btn btn--chip"
                  type="button"
                  disabled={busy || changeBlocked}
                  data-testid="mission-move-{to}"
                  onclick={() => void move(to)}>{moveLabel(mission.state, to)}</button
                >
              {/each}
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
          </div>
          {#if confirmMove}
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
        {/if}
      {/if}

      {#if plan}
        <section class="loop" data-testid="mission-loop">
          <p class="muted small" data-testid="mission-autonomy">
            {autonomy?.runs} · {autonomy?.limits} · spent {dollars(plan.cost_micros)}{#if plan.autonomy.grant?.budget_micros}
              of {dollars(plan.autonomy.grant.budget_micros)}{/if} · {plan.counts.open} running
          </p>
          {#if autonomy?.hint}<p class="muted small" data-testid="mission-autonomy-hint">{autonomy.hint}</p>{/if}
          {#if mayChange}
            <div class="row">
              {#if mission.state === 'active' && pressable.length > 0}
                <!-- Redesign 1.5: the mission's one primary (the edit form's
                     Save takes over while it is open). -->
                <button class="btn" class:btn--primary={!editing} type="button" disabled={busy || changeBlocked} data-testid="mission-start-wave" onclick={() => void startWave()}
                  >Start wave ({pressable.length})</button
                >
              {/if}
              <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-plan" onclick={() => void askPlanner()}
                >Ask the planner</button
              >
              {#if plan.autonomy.grant}
                <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-revoke" onclick={() => void revokeGrant()}
                  >End the grant (L{plan.autonomy.grant.level}, until {new Date(plan.autonomy.grant.expires_at * 1000).toLocaleString()})</button
                >
              {:else}
                <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-grant" onclick={() => (granting = !granting)}
                  >Grant…</button
                >
              {/if}
            </div>
            {#if granting}
              <form class="row" data-testid="mission-grant-form" onsubmit={(e) => (e.preventDefault(), void grant())}>
                <label class="field inline"
                  >Level
                  <select bind:value={grantLevel}>
                    <option value={1}>L1 · You press every step</option>
                    <option value={2}>L2 · runs, retries, reviews</option>
                    <option value={3}>L3 · also creates tasks</option>
                  </select></label
                >
                <label class="field inline">Hours <input type="number" min="1" max="168" bind:value={grantHours} class="role" /></label>
                <label class="field inline">Budget $ <input placeholder="none" bind:value={grantBudget} class="role" data-testid="mission-grant-budget" /></label>
                {#if $hosts.length > 0}
                  <fieldset class="field inline hosts" data-testid="mission-grant-hosts">
                    <legend>Hosts <span class="muted small">(none ticked: any)</span></legend>
                    {#each $hosts as h (h.alias)}
                      <label class="host"
                        ><input type="checkbox" value={h.alias} bind:group={grantHosts} data-testid="mission-grant-host" /> {h.alias}</label
                      >
                    {/each}
                  </fieldset>
                {/if}
                <button class="btn" type="submit" disabled={busy} data-testid="mission-grant-save">Sign</button>
              </form>
            {/if}
          {/if}
          {#if plannerFailure}
            <div class="planner-error" role="alert" data-testid="mission-planner-error">
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
          {#if (plan.steps ?? []).length > 0}
            <ul class="steps" aria-label="Next steps" data-testid="mission-steps">
              {#each plan.steps ?? [] as st (stepKey(st))}
                <li data-testid="mission-step" data-kind={st.kind}>
                  <span>{stepLine(st)}</span>
                  {#if mayChange && mission.state === 'active' && st.kind !== 'ask'}
                    <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} onclick={() => void startWave(stepKey(st))}>Go</button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
          {#if openCards.length > 0}
            <h4>Waiting for you</h4>
            <ul class="cards" data-testid="mission-cards">
              {#each openCards as c (c.id)}
                <li data-testid="mission-card" data-kind={c.kind}>
                  <span class="muted small">{c.source === 'planner' ? 'Planner' : 'Fleet'}</span>
                  <span>{cardLine(c)}</span>
                  {#if detail.may_change}
                    {#if c.kind === 'ask'}
                      <input placeholder="Your answer" bind:value={answers[c.id]} data-testid="mission-card-answer" />
                    {/if}
                    <button class="btn btn--chip" type="button" disabled={busy || changeBlocked} data-testid="mission-card-apply" onclick={() => void decide(c, true)}
                      >{c.kind === 'ask' ? 'Answer' : 'Apply'}</button
                    >
                    <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="mission-card-dismiss" onclick={() => void decide(c, false)}
                      >Dismiss</button
                    >
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      {/if}

      <h4>Tasks</h4>
      {#if mayChange}
        <div class="import" data-testid="mission-import">
          {#if !importOpen}
            <button class="btn btn--quiet" type="button" disabled={busy || importBlocked} data-testid="mission-import-open" onclick={() => (importOpen = true)}
              >Import plan…</button
            >
          {:else}
            <p class="muted small">
              Paste a markdown plan. Fleet reads its step tables (#, Step, Needs, and Lane or Status when there) and a Lanes table
              (Lane, Steps in order). Each step becomes a task here; importing again updates them.
            </p>
            <textarea rows="6" bind:value={importText} placeholder={'| # | Step | Needs |\n|---|---|---|\n| 1.1 | Schema | — |\n| 1.2 | API | 1.1 |'} data-testid="mission-import-text"
            ></textarea>
            {#if parsed}
              <p class="muted small" data-testid="mission-import-preview">
                {parsed.rows.length} steps · {parsedLanes} lanes · {parsedLinks} links
                {#if parsed.rows.length > PLAN_IMPORT_MAX_ROWS} · at most {PLAN_IMPORT_MAX_ROWS} at once{/if}
              </p>
              {#each parsed.notes as n (n)}<p class="muted small">{n}</p>{/each}
            {/if}
            <span class="row">
              <button
                class="btn"
                type="button"
                disabled={busy || importBlocked || !parsed || parsed.rows.length === 0 || parsed.rows.length > PLAN_IMPORT_MAX_ROWS}
                data-testid="mission-import-run"
                onclick={() => void runImport()}>Import {parsed?.rows.length ?? 0} steps</button
              >
              <button class="btn btn--quiet" type="button" onclick={() => ((importOpen = false), (importText = ''))}>Cancel</button>
            </span>
            {#if mission.mode !== 'plan'}
              <p class="muted small">A {mission.mode} mission holds 30 tasks; make it a Plan to hold up to 200.</p>
            {/if}
          {/if}
          {#if importResult}
            <p class="muted small" role="status" data-testid="mission-import-result">{importResult}</p>
          {/if}
          {#if importUnknown.length > 0}
            <p class="muted small" data-testid="mission-import-unknown">Needs that name no step: {importUnknown.join(', ')}</p>
          {/if}
        </div>
      {/if}
      {#if mayChange && proposals.length > 0}
        <div class="row proposals" data-testid="mission-proposals">
          <span>{proposals.length} proposed {proposals.length === 1 ? 'task waits' : 'tasks wait'} for you.</span>
          <button class="btn btn--chip" type="button" disabled={busy} data-testid="mission-accept-all" onclick={() => void acceptAll()}
            >Accept all</button
          >
        </div>
      {/if}
      {#if lastAccepted.length > 0}
        <div class="row" role="status" data-testid="mission-accepted">
          <span>Accepted {lastAccepted.length}.</span>
          <button class="btn btn--quiet" type="button" disabled={busy} data-testid="mission-undo-accept" onclick={() => void undoAccept()}
            >Undo</button
          >
        </div>
      {/if}
        <div class="view-switch" role="tablist" aria-label="Show tasks as" use:tablistKeys>
          <button
            type="button"
            role="tab"
            aria-selected={tasksView === 'list'}
            data-testid="mission-view-list"
            onclick={() => (tasksView = 'list')}>List</button
          >
          <button
            type="button"
            role="tab"
            aria-selected={tasksView === 'graph'}
            data-testid="mission-view-graph"
            onclick={() => (tasksView = 'graph')}>Graph</button
          >
        </div>
      {#if showGraph && detail}
        <MissionGraph
          {detail}
          {laneBy}
          {repoName}
          onlanechange={(by) => {
            if (mission) laneOverride = { mission: mission.id, by };
          }}
        />
      {/if}
      {#each showGraph ? [] : waves as w (w.wave)}
        <div class="wave" data-testid="mission-wave">
          {#if waves.length > 1}<span class="wave-head">W{w.wave}</span>{/if}
          <ul class="items" data-testid="mission-items">
            {#each w.nodes as n (n.item_id)}
              {@const it = itemById.get(n.item_id)}
              {#if it}
                <li data-testid="mission-node" data-state={n.state}>
                  <span class="node-dot" title={nodeLabel(n.state)}><StatusDot state={toneOf(n.state)} label={nodeLabel(n.state)} /></span>
                  {#if trails.has(n.item_id)}
                    <Loader name="comet-trails" size={20} label="Working on it" testid="mission-trails" />
                  {/if}
                  <span class="main">
                    <span class="title">{it.key ? `${it.key} · ` : ''}{it.title}</span>
                    {#if (n.waiting_for ?? []).length > 0}
                      <span class="muted small" data-testid="mission-waits">waits for {(n.waiting_for ?? []).map(titleOf).join(', ')}</span>
                    {/if}
                    {#if n.attempt}
                      <span class="muted small" data-testid="mission-attempt" title={n.attempt.summary ?? ''}
                        >{attemptLine(n.attempt)}</span
                      >
                    {/if}
                    {#if n.verification}
                      <span class="vbadge v-{n.verification.state}" data-testid="mission-verified"
                        >{verificationLabel(n.verification.state)}</span
                      >
                      <ul class="checks" data-testid="mission-checks">
                        {#each n.verification.checks as c (c.line)}
                          <li class="c-{c.state}">
                            <span class="glyph" aria-label={c.state}>{checkGlyph(c.state)}</span>
                            <span class="line">{c.line}</span>
                            <span class="muted small">{c.detail}</span>
                            {#if detail?.may_change && checkable(c)}
                              <button
                                class="btn btn--chip"
                                type="button"
                                disabled={busy}
                                data-testid="mission-check"
                                onclick={() => void check(n.item_id, c.line, true)}>Met</button
                              >
                            {/if}
                          </li>
                        {/each}
                      </ul>
                    {/if}
                    {#if condsFor === n.item_id}
                      <form class="conds" onsubmit={(e) => (e.preventDefault(), void saveConds())}>
                        <textarea
                          rows="3"
                          placeholder={'ci\nreview\ntest:cargo test\nperson'}
                          bind:value={condText}
                          data-testid="mission-conds-text"
                        ></textarea>
                        <span class="row">
                          <button class="btn" type="submit" disabled={busy} data-testid="mission-conds-save">Save</button>
                          <button class="btn btn--quiet" type="button" onclick={() => (condsFor = null)}>Cancel</button>
                        </span>
                      </form>
                    {/if}
                    {#if mayChange && (n.depends_on ?? []).length > 0}
                      <span class="deps">
                        {#each n.depends_on ?? [] as d (d)}
                          <button
                            class="btn btn--chip dep"
                            type="button"
                            title="Stop waiting for {titleOf(d)}"
                            disabled={busy}
                            data-testid="mission-dep-remove"
                            onclick={() => void setDep(n.item_id, d, false)}>after {titleOf(d)} ×</button
                          >
                        {/each}
                      </span>
                    {/if}
                  </span>
                  {#if mayChange && condsFor !== n.item_id}
                    <button
                      class="btn btn--quiet btn--icon"
                      type="button"
                      aria-label="Done when…"
                      title="Done when: the conditions that verify this task"
                      disabled={busy}
                      data-testid="mission-conds"
                      onclick={() => editConds(n.item_id)}><Icon name="checklist" size={14} /></button
                    >
                  {/if}
                  {#if it.id === mission.root_item_id}
                    <span class="muted small">root</span>
                  {:else if mayChange}
                    {@const choices = depChoices(n)}
                    {#if choices.length > 0}
                      <select
                        class="dep-pick"
                        aria-label="Waits for"
                        disabled={busy}
                        data-testid="mission-dep-add"
                        onchange={(e) => {
                          const v = Number((e.currentTarget as HTMLSelectElement).value);
                          (e.currentTarget as HTMLSelectElement).value = '';
                          if (v) void setDep(n.item_id, v, true);
                        }}
                      >
                        <option value="">after…</option>
                        {#each choices as c (c.id)}<option value={c.id}>{c.title}</option>{/each}
                      </select>
                    {/if}
                    {#if n.state === 'failed' && mission.state === 'active'}
                      <button
                        class="btn btn--quiet btn--icon"
                        type="button"
                        aria-label="Retry"
                        title="Try again, with the last failure in the prompt"
                        disabled={busy || changeBlocked}
                        data-testid="mission-retry"
                        onclick={() => void retry(it.id)}>↻</button
                      >
                    {/if}
                    {#if n.state !== 'done'}
                      <button
                        class="btn btn--quiet btn--icon"
                        type="button"
                        aria-label={it.held_at ? 'Release' : 'Hold'}
                        title={it.held_at ? 'Release' : 'Hold: never start this on its own'}
                        disabled={busy}
                        data-testid="mission-hold"
                        onclick={() => void setHold(it.id, !it.held_at)}><Icon name={it.held_at ? 'play' : 'pause'} size={14} /></button
                      >
                    {/if}
                    <button
                      class="btn btn--quiet btn--icon"
                      type="button"
                      aria-label="Take out of the mission"
                      title="Take out of the mission"
                      disabled={busy}
                      onclick={() => void removeItem(it.id)}>×</button
                    >
                  {/if}
                </li>
              {/if}
            {/each}
          </ul>
        </div>
      {/each}
      {#if (detail.graph?.outside ?? []).length > 0}
        <p class="muted small" data-testid="mission-outside">
          Waits on work outside the mission: {(detail.graph?.outside ?? []).map((o) => (o.key ? `${o.key} · ${o.title}` : o.title)).join(', ')}
        </p>
      {/if}
      {#if mayChange && mission.root_item_id}
        <form class="row" onsubmit={(e) => (e.preventDefault(), void addTask())}>
          <input placeholder="New task" bind:value={newTask} data-testid="mission-new-task" />
          <button class="btn" type="submit" disabled={busy || !newTask.trim()}>Add</button>
        </form>
      {/if}

      <h4>Repos</h4>
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
                  onclick={() => void removeRepo(r.project_id)}>×</button
                >
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      {#if mayChange && repoChoices.length > 0}
        <div class="row">
          <select bind:value={repoPick} data-testid="mission-repo-pick">
            <option value="">Add a repo…</option>
            {#each repoChoices as p (p.id)}<option value={p.id}>{p.owner}/{p.repo}</option>{/each}
          </select>
          <input class="role" placeholder="role" bind:value={repoRole} />
          <button class="btn" type="button" disabled={busy || repoPick === ''} data-testid="mission-repo-add" onclick={() => void addRepo()}
            >Add</button
          >
        </div>
      {/if}

      {#if detail.may_change && mission.state === 'completed'}
        <!-- Redesign 9.11: Finish's release note, drafted on demand. -->
        <h4>Release note</h4>
        <ReleaseNote missionId={mission.id} />
      {/if}

      <h4>Log</h4>
      <ul class="events" data-testid="mission-events">
        {#each detail.events ?? [] as e (e.id)}
          <li><span class="muted small">{shortAge(e.at)}</span> {eventSentence(e)}</li>
        {/each}
      </ul>

      {#if detail.may_change && (mission.state === 'draft' || isFinal(mission.state))}
        {#if confirmDelete}
          <div class="row">
            <span>Delete {mission.name}? Its tasks stay.</span>
            <button class="btn btn--danger" type="button" disabled={busy} data-testid="mission-delete-confirm" onclick={() => void remove()}
              >Delete</button
            >
            <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = false)}>Keep</button>
          </div>
        {:else}
          <button class="btn btn--quiet" type="button" data-testid="mission-delete" onclick={() => (confirmDelete = true)}>Delete…</button>
        {/if}
      {/if}
    </div>
  {:else}
    <div class="bar">
      {#if creating}
        <form class="create" onsubmit={(e) => (e.preventDefault(), void create())}>
          <input placeholder="Name" bind:value={newName} data-testid="mission-new-name" />
          <textarea rows="3" placeholder="Goal: what is true when it is done" bind:value={newGoal} data-testid="mission-new-goal"
          ></textarea>
          <div class="row">
            <button class="btn btn--primary" type="submit" disabled={busy || saveBlocked} data-testid="mission-create">Create</button>
            <button class="btn btn--quiet" type="button" onclick={() => (creating = false)}>Cancel</button>
          </div>
        </form>
      {:else}
        <button class="btn btn--primary" type="button" disabled={saveBlocked} data-testid="mission-new" onclick={() => (creating = true)}>New mission</button>
        {#if missions.some((m) => m.state === 'active')}
          <button class="btn btn--quiet" type="button" disabled={busy || changeBlocked} data-testid="missions-pause-all" onclick={() => void pauseAll()}
            >Pause all</button
          >
        {/if}
      {/if}
    </div>
    {#if error}
      <div class="state error" role="alert" data-testid="missions-error">
        <p>{error}</p>
        <button class="btn" type="button" onclick={() => void load()}>Retry</button>
      </div>
    {:else if !loaded}
      <div data-testid="missions-loading"><Skeleton rows={3} label="Loading missions" /></div>
    {:else if missions.length === 0}
      <p class="muted" data-testid="missions-empty">
        No missions yet. A mission is a goal with the tasks that reach it; create one to gather the work.
      </p>
    {:else}
      <ul class="list" aria-label="Missions">
        {#each missions as m (m.id)}
          <li>
            <button class="open" type="button" data-testid="mission-row" onclick={() => void open(m.id)}>
              <span class="title">{m.name}</span>
              <span class="badge">{stateLabel(m.state)}</span>
              {#if progressLabel(m)}<span class="muted small">{progressLabel(m)}</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .loop { display: flex; flex-direction: column; gap: 0.3rem; border-left: 2px solid var(--border); padding-left: 0.5rem; }
  .steps, .cards { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; }
  .steps li, .cards li { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .cards input { flex: 1 1 8rem; min-width: 0; }
  .missions { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.5rem; font-size: var(--text-xs); }
  .more { position: relative; display: inline-flex; }
  .more-menu {
    position: absolute; top: calc(100% + 4px); left: 0; z-index: 30; min-width: 11rem; padding: 0.25rem;
    display: flex; flex-direction: column; background: var(--bg); border: 1px solid var(--border);
    border-radius: var(--radius-md); box-shadow: var(--shadow-pop);
  }
  .mi {
    text-align: left; border: none; background: transparent; color: var(--fg); cursor: pointer;
    font: inherit; font-size: var(--text-xs); padding: 0.35rem 0.5rem; border-radius: var(--radius-sm);
  }
  .mi:hover, .mi:focus-visible { background: var(--bg-hover); }
  .mi.danger { color: var(--danger); }
  .btn--chip.danger { color: var(--danger); }
  .confirm-move { margin-top: 0.3rem; font-size: var(--text-xs); }
  .bar, .row { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .create { display: flex; flex-direction: column; gap: 0.4rem; width: 100%; }
  input, textarea, select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .row input { flex: 1 1 8rem; min-width: 0; }
  .row input.role { flex: 0 1 6rem; }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  .field.inline { flex: 1 1 10rem; }
  ul { list-style: none; margin: 0; padding: 0; }
  .list li, .items li, .repos li { border-bottom: 1px solid var(--border); }
  .items li, .repos li { display: flex; gap: 0.4rem; align-items: center; padding: 0.25rem 0; }
  .open {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    width: 100%;
    padding: 0.35rem 0.2rem;
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .open:hover { background: var(--bg-hover); }
  .title { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .name { margin: 0.25rem 0 0; overflow-wrap: anywhere; }
  .goal { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  .badge { font-size: var(--text-2xs); padding: 0.05rem 0.4rem; border: 1px solid var(--border); border-radius: var(--radius-pill); white-space: nowrap; }
  h4 { margin: 0.5rem 0 0; font-size: var(--text-2xs); text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  .done-when li { padding: 0.1rem 0; }
  .events li { padding: 0.1rem 0; }
  .glyph { width: 1.1rem; text-align: center; flex: 0 0 auto; color: var(--fg-muted); }
  .node-dot { display: inline-flex; align-items: center; height: 1lh; flex: 0 0 auto; }
  .main { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; }
  .deps { display: flex; flex-wrap: wrap; gap: 0.2rem; margin-top: 0.15rem; }
  .dep { font-size: var(--text-2xs); }
  .dep-pick { max-width: 7rem; font-size: var(--text-2xs); }
  .import { display: flex; flex-direction: column; gap: 0.3rem; margin-bottom: 0.4rem; }
  .import textarea { font-family: var(--font-mono); font-size: var(--text-xs); }
  /* The design system's Tabs (of-tabs). */
  .view-switch { display: flex; gap: 20px; border-bottom: 1px solid var(--border); margin-bottom: 0.4rem; }
  .view-switch button {
    font: inherit; font-size: var(--text-sm); color: var(--fg-muted); background: none; border: 0; cursor: pointer;
    padding: 6px 0; border-bottom: 2px solid transparent; margin-bottom: -1px;
  }
  .view-switch button:hover { color: var(--fg); }
  .view-switch button[aria-selected='true'] { color: var(--fg); border-bottom-color: var(--accent); font-weight: 500; }
  .wave { display: flex; flex-direction: column; gap: 0.1rem; }
  .wave-head { font-size: var(--text-2xs); color: var(--fg-muted); margin-top: 0.3rem; }
  .vbadge { font-size: var(--text-2xs); align-self: flex-start; padding: 0 0.35rem; border: 1px solid var(--border); border-radius: var(--radius-pill); }
  .vbadge.v-verified { color: var(--status-done); border-color: var(--status-done); }
  .vbadge.v-failed { color: var(--danger); border-color: var(--danger); }
  .checks li { display: flex; gap: 0.3rem; align-items: baseline; border: none; padding: 0; font-size: var(--text-2xs); }
  .checks .line { font-family: var(--font-mono); }
  .checks .c-pass .glyph { color: var(--status-done); }
  .checks .c-fail .glyph { color: var(--danger); }
  .conds { display: flex; flex-direction: column; gap: 0.2rem; margin-top: 0.2rem; }
  .proposals { padding: 0.3rem 0.4rem; border: 1px dashed var(--border); border-radius: var(--radius-sm); }
  .muted { color: var(--fg-muted); margin: 0; }
  .small { font-size: var(--text-2xs); }
  .meta { font-size: var(--text-2xs); }
  .notice { margin: 0; color: var(--danger); }
  .state.error p { color: var(--danger); margin: 0 0 0.3rem; }
  .hosts { border: none; padding: 0; margin: 0; display: flex; flex-wrap: wrap; gap: var(--space-2); align-items: center; }
  .hosts legend { padding: 0; margin-right: var(--space-1); float: left; }
  .planner-error { border: 1px solid var(--danger); border-radius: var(--radius-md); padding: var(--space-2) var(--space-3); margin: var(--space-2) 0; }
  .planner-error p { margin: 0 0 var(--space-1); }
  .planner-error .title { color: var(--danger); font-weight: 600; }
  .planner-error .details { margin: var(--space-1) 0 0; white-space: pre-wrap; word-break: break-word; font-size: var(--text-xs); color: var(--fg-muted); }
</style>
