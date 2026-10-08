<script lang="ts">
  // Missions (orchestration O1, design 2026-10-07 §9): the Work view's third
  // tab. A mission is a goal over a root task: its member tasks, the repos
  // it may run in, its lifecycle and its log. Nothing runs on its own yet
  // (the loop is O4/O5); this is where a person writes the goal down and
  // gathers the work.
  //
  // Text a person or a tracker wrote (names, goals, task titles) is rendered
  // as text, never as markup.
  import { onDestroy, onMount } from 'svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { projects } from './projects';
  import { timeAgo } from './session_status';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import { readErrorText } from './work_view';
  import {
    MISSION_MOVES,
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
    nodeGlyph,
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
    type GraphNode,
    type Mission,
    type MissionDetail,
  } from './missions';

  let missions = $state.raw<Mission[]>([]);
  let loaded = $state(false);
  let error = $state<string | null>(null);
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

  let newTask = $state('');
  let repoPick = $state<number | ''>('');
  let repoRole = $state('');
  let confirmDelete = $state(false);

  const saveBlocked = $derived(!!hubActionBlocked('save_mission', $hubStatus, $hubConnection));
  const changeBlocked = $derived(!!hubActionBlocked('set_mission_state', $hubStatus, $hubConnection));

  const mission = $derived(detail?.mission ?? null);
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
  const mayChange = $derived(!!detail?.may_change && !!mission && !isFinal(mission.state));
  const moves = $derived(mission ? (MISSION_MOVES[mission.state] ?? []) : []);
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

  async function loadDetail(id: number) {
    const r = await getMission(id);
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
    notice = null;
    await loadDetail(id);
  }

  function back() {
    selectedId = null;
    detail = null;
    editing = false;
    notice = null;
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
    editing = true;
  }

  async function saveEdit() {
    if (!mission) return;
    const m = await act(
      updateMission(
        mission.id,
        {
          goal: editGoal,
          non_goals: editNonGoals,
          done_when: doneWhenRows(editDoneWhen),
          level: editLevel,
          mode: editMode,
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
        {#if mission.mode === 'continuous'}Continuous · {/if}L{mission.level}
        {#if progressLabel(mission)} · {progressLabel(mission)}{/if}
        · updated {timeAgo(mission.updated_at)}
      </p>

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
              <option value={1}>L1 · I press Start</option>
              <option value={2}>L2 · runs within a grant</option>
              <option value={3}>L3 · reviews and integrates</option>
            </select></label
          >
          <label class="field inline"
            >Mode
            <select bind:value={editMode}>
              <option value="finite">Finite</option>
              <option value="continuous">Continuous</option>
            </select></label
          >
        </div>
        <div class="row">
          <button class="btn" type="button" disabled={busy || saveBlocked} data-testid="mission-edit-save" onclick={() => void saveEdit()}
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
            {#each moves as to (to)}
              <button
                class="btn btn--chip"
                type="button"
                disabled={busy || changeBlocked}
                data-testid="mission-move-{to}"
                onclick={() => void move(to)}>{moveLabel(mission.state, to)}</button
              >
            {/each}
          </div>
        {/if}
      {/if}

      <h4>Tasks</h4>
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
      {#each waves as w (w.wave)}
        <div class="wave" data-testid="mission-wave">
          {#if waves.length > 1}<span class="wave-head">W{w.wave}</span>{/if}
          <ul class="items" data-testid="mission-items">
            {#each w.nodes as n (n.item_id)}
              {@const it = itemById.get(n.item_id)}
              {#if it}
                <li data-testid="mission-node" data-state={n.state}>
                  <span class="glyph s-{n.state}" title={nodeLabel(n.state)} aria-label={nodeLabel(n.state)}>{nodeGlyph(n.state)}</span>
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
                      onclick={() => editConds(n.item_id)}>☑</button
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
                    {#if n.state !== 'done'}
                      <button
                        class="btn btn--quiet btn--icon"
                        type="button"
                        aria-label={it.held_at ? 'Release' : 'Hold'}
                        title={it.held_at ? 'Release' : 'Hold: never start this on its own'}
                        disabled={busy}
                        data-testid="mission-hold"
                        onclick={() => void setHold(it.id, !it.held_at)}>{it.held_at ? '▶' : '⏸'}</button
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

      <h4>Log</h4>
      <ul class="events" data-testid="mission-events">
        {#each detail.events ?? [] as e (e.id)}
          <li><span class="muted small">{timeAgo(e.at)}</span> {eventSentence(e)}</li>
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
            <button class="btn" type="submit" disabled={busy || saveBlocked} data-testid="mission-create">Create</button>
            <button class="btn btn--quiet" type="button" onclick={() => (creating = false)}>Cancel</button>
          </div>
        </form>
      {:else}
        <button class="btn" type="button" disabled={saveBlocked} data-testid="mission-new" onclick={() => (creating = true)}>New mission</button>
      {/if}
    </div>
    {#if error}
      <div class="state error" role="alert" data-testid="missions-error">
        <p>{error}</p>
        <button class="btn" type="button" onclick={() => void load()}>Retry</button>
      </div>
    {:else if !loaded}
      <p class="muted" data-testid="missions-loading">Loading missions…</p>
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
  .missions { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.5rem; font-size: 0.85rem; }
  .bar, .row { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .create { display: flex; flex-direction: column; gap: 0.4rem; width: 100%; }
  input, textarea, select {
    font: inherit;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
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
  .open:hover { background: var(--bg-hover, rgba(127, 127, 127, 0.08)); }
  .title { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .name { margin: 0.25rem 0 0; overflow-wrap: anywhere; }
  .goal { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  .badge { font-size: 0.75rem; padding: 0.05rem 0.4rem; border: 1px solid var(--border); border-radius: 999px; white-space: nowrap; }
  h4 { margin: 0.5rem 0 0; font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-muted); }
  .done-when li { padding: 0.1rem 0; }
  .events li { padding: 0.1rem 0; }
  .glyph { width: 1.1rem; text-align: center; flex: 0 0 auto; color: var(--fg-muted); }
  .glyph.s-done, .glyph.s-ready { color: #3fae5a; }
  .glyph.s-running, .glyph.s-doing { color: #e0a030; }
  .glyph.s-failed, .glyph.s-blocked { color: #e64a4a; }
  .main { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; }
  .deps { display: flex; flex-wrap: wrap; gap: 0.2rem; margin-top: 0.15rem; }
  .dep { font-size: 0.7rem; }
  .dep-pick { max-width: 7rem; font-size: 0.75rem; }
  .wave { display: flex; flex-direction: column; gap: 0.1rem; }
  .wave-head { font-size: 0.7rem; color: var(--fg-muted); margin-top: 0.3rem; }
  .vbadge { font-size: 0.7rem; align-self: flex-start; padding: 0 0.35rem; border: 1px solid var(--border); border-radius: 999px; }
  .vbadge.v-verified { color: #3fae5a; border-color: #3fae5a; }
  .vbadge.v-failed { color: #e64a4a; border-color: #e64a4a; }
  .checks li { display: flex; gap: 0.3rem; align-items: baseline; border: none; padding: 0; font-size: 0.75rem; }
  .checks .line { font-family: var(--font-mono, monospace); }
  .checks .c-pass .glyph { color: #3fae5a; }
  .checks .c-fail .glyph { color: #e64a4a; }
  .conds { display: flex; flex-direction: column; gap: 0.2rem; margin-top: 0.2rem; }
  .proposals { padding: 0.3rem 0.4rem; border: 1px dashed var(--border); border-radius: 4px; }
  .muted { color: var(--fg-muted); margin: 0; }
  .small { font-size: 0.75rem; }
  .meta { font-size: 0.75rem; }
  .notice { margin: 0; color: #e64a4a; }
  .state.error p { color: #e64a4a; margin: 0 0 0.3rem; }
</style>
