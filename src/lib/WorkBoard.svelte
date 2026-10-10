<script lang="ts">
  import { viewKey } from './shortcuts';
  import Skeleton from './states/Skeleton.svelte';
  import Icon from './kit/Icon.svelte';
  // The task board (sprints design 2026-09-28 §6c): every task the Work
  // view's filters match, from one `work_tree` read (archived on, so Done
  // has its rows; the status filter off, since the columns are the status).
  // The columns come from the trackers (redesign 6.1): To do, In progress
  // and Done, and every status name a shown ticket's tracker reports
  // (Backlog, In Review, an Asana section) beside the status it belongs to,
  // the List's mapping (`groupTasksForBoard`). A native task's card drags to
  // another status's column — the one place a status is set by dragging, a
  // person's setting that is final over the derived one — and lands in that
  // status's own column. A tracker's card sits where its tracker reports
  // and says so when dragged (E11): no tracker takes a status back yet
  // (`Caps.write` is false for all), so none moves. A card shows its live
  // session and host.
  //
  // Scope (sprints design §6c): every task (the default), one open sprint —
  // its tasks only, its roll-up, goal and dates above the columns, Start /
  // Close sprint… there, and Done holding everything it delivered rather
  // than the last week — or the tasks in no sprint, the backlog a sprint is
  // planned from. One `work_tree` read either way: a sprint is the section
  // of a group by sprint (`boardFilters`).
  //
  // The drag is pointer events, not HTML5 drag and drop: the window takes
  // OS file drops (`dragDropEnabled`), which on Windows swallows the
  // webview's own. ← / → on a focused card move it too. Tracker and agent
  // text renders as text. A native card's edit button (or E on the focused card)
  // opens the edit dialog: title, description, status, assignees.
  //
  // Gap plan G3.5: a status's own column has "+" to add a task in that
  // status; x selects cards (Shift+click too) and the selection bar moves
  // them together; s starts a new session for the focused card; the card
  // shows its pull request with its checks, and the open card its Work
  // button (Start new, Continue ▾).
  import { onDestroy, onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { createWorkTask, onWorkChangedDebounced, ownerDueChip, setWorkStatus } from './work';
  import { sessionAgent, sessions } from './sessions';
  import { AGENT_LABELS } from './row_groups';
  import { prChip } from './work_row';
  import { workButtonFor } from './start_preview';
  import WorkButton from './WorkButton.svelte';
  import {
    openTask,
    readErrorText,
    selectedTaskId,
    sidebarView,
    workTree,
    workViewFilters,
    activeWorkViewId,
    type WorkTask,
  } from './work_view';
  import { facetSentence, workFacets } from './filter_facets';
  import { providerInfo } from './trackers';
  import { hintAnchor } from './hints';
  import EditTaskDialog from './EditTaskDialog.svelte';
  import WorkBuckets from './WorkBuckets.svelte';
  import {
    advanceBucket,
    boardFilters,
    boardScope,
    bucketSummary,
    liveScope,
    openBuckets,
    unixToDay,
    workBuckets,
    type BoardScope,
    type BucketRow,
  } from './work_buckets';
  import TaskBlockedSpend from './TaskBlockedSpend.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    BOARD_COLUMN_STATUS,
    boardLaneOf,
    boardLiveSession,
    boardMoveRefusal,
    boardStep,
    displayTitle,
    groupTasksForBoard,
    type BoardLane,
    type TaskNode,
  } from './task_list';
  import type { IpcError } from './result';

  let {
    onclose,
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
    /** The longest a steady stream of changes may hold a refetch back, ms. */
    maxWaitMs = 3000,
  }: { onclose?: () => void; debounceMs?: number; maxWaitMs?: number } = $props();

  let tasks = $state.raw<WorkTask[]>([]);
  const byId = $derived(new Map(tasks.map((t) => [t.task_id, t])));
  const taskById = (id: string) => byId.get(id);
  let more = $state(false);
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  /** Cards a person moved (task → lane id), placed before the hub answers. */
  let overrides = $state.raw<Map<string, string>>(new Map());
  /** Cards moved on this board: kept on Done past its window. */
  let moved = $state.raw<Set<string>>(new Set());
  /** A refused or failed move, on the card it was about. */
  let cardErrors = $state.raw<Map<string, string>>(new Map());

  /** The task whose edit dialog is open. */
  let editing = $state<string | null>(null);
  const editBlocked = $derived(hubActionBlocked('edit_work_item', $hubStatus, $hubConnection));
  const moveBlocked = $derived(hubActionBlocked('set_work_status', $hubStatus, $hubConnection));
  /** The sprints, once read (`null` before). */
  let sprints = $state.raw<BucketRow[] | null>(null);
  const scope = $derived(liveScope($boardScope, sprints));
  const sprint = $derived(typeof scope === 'number' ? (sprints ?? []).find((b) => b.id === scope) : undefined);
  const openSprints = $derived(openBuckets(sprints ?? [], 'sprint'));
  const adminBlocked = $derived(hubActionBlocked('work_bucket_admin', $hubStatus, $hubConnection));
  let closingSprint = $state<number | null>(null);
  let scopeNotice = $state<string | null>(null);
  const columns = $derived(
    groupTasksForBoard(tasks, Math.floor(Date.now() / 1000), overrides, moved, typeof scope !== 'number'),
  );
  /** Some column is a tracker's own name: say where the columns come from. */
  const trackerLanes = $derived(columns.lanes.some((l) => l.id !== l.status));
  const laneById = (id: string | undefined) => columns.lanes.find((l) => l.id === id);
  /** The column a card shows in now: a pending move's, else its own. */
  const laneOfCard = (t: WorkTask): BoardLane => laneById(overrides.get(t.task_id)) ?? boardLaneOf(t);

  let sprintSeq = 0;
  async function loadSprints() {
    const mine = ++sprintSeq;
    const r = await workBuckets('sprint');
    if (mine !== sprintSeq) return;
    // An older hub (or a refusal) leaves the board unscoped, as before.
    sprints = r.ok && Array.isArray(r.value) ? r.value : [];
  }

  let seq = 0;
  /** The scope the tasks shown were read for. */
  let readScope: BoardScope = 'all';
  async function load() {
    const mine = ++seq;
    const want = liveScope(get(boardScope), sprints);
    const r = await workTree({ filters: boardFilters(get(workViewFilters), want), limit: 200, per_task: 3 });
    if (mine !== seq) return;
    loaded = true;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
    readScope = want;
    more = !!r.value?.next_cursor;
    // What the hub now says wins over a placement made before it answered.
    overrides = new Map();
  }

  let lastFilters = JSON.stringify(get(workViewFilters));
  const offFilters = workViewFilters.subscribe((f) => {
    const k = JSON.stringify(f);
    if (k === lastFilters) return;
    lastFilters = k;
    void load();
  });
  // A sprint chosen, or one that closed under the board: read again.
  $effect(() => {
    if (loaded && scope !== readScope) void load();
  });
  const offChanged = onWorkChangedDebounced(
    () => {
      void loadSprints();
      void load();
    },
    () => debounceMs,
    () => maxWaitMs,
  );
  onMount(() => {
    void loadSprints();
    void load();
  });
  onDestroy(() => {
    offFilters();
    offChanged();
    endDrag();
  });

  function setCardError(id: string, msg: string | null) {
    const next = new Map(cardErrors);
    if (msg) next.set(id, msg);
    else next.delete(id);
    cardErrors = next;
  }

  /** Move a card to `to`: refused on the card for a task fleet does not own
   *  the status of, else placed at once and set on the hub. A native task
   *  lands in the status's own column, whichever column of that status it
   *  was dropped on. */
  async function move(t: WorkTask, to: BoardLane) {
    const from = laneOfCard(t);
    if (from.id === to.id) return;
    const refusal = boardMoveRefusal(t);
    if (refusal) {
      setCardError(t.task_id, refusal);
      return;
    }
    if (moveBlocked) {
      setCardError(t.task_id, moveBlocked);
      return;
    }
    if (from.status === to.status) return;
    setCardError(t.task_id, null);
    overrides = new Map(overrides).set(t.task_id, to.status);
    moved = new Set(moved).add(t.task_id);
    const r = await setWorkStatus(t.item_id as number, BOARD_COLUMN_STATUS[to.status]);
    if (!r.ok) {
      const back = new Map(overrides);
      back.delete(t.task_id);
      overrides = back;
      setCardError(t.task_id, readErrorText(r.error));
    }
  }

  // ── Add a task in a column (G3.5) ──
  let addingIn = $state<string | null>(null);
  let addTitle = $state('');
  let addBusy = $state(false);
  let addError = $state<string | null>(null);
  async function addIn(lane: BoardLane) {
    const title = addTitle.trim();
    if (!title || addBusy) return;
    addBusy = true;
    addError = null;
    const r = await createWorkTask({ title });
    if (r.ok && BOARD_COLUMN_STATUS[lane.status] !== 'todo') {
      const s = await setWorkStatus(r.value.id, BOARD_COLUMN_STATUS[lane.status]);
      if (!s.ok) addError = `Added to To do: ${readErrorText(s.error)}`;
    }
    addBusy = false;
    if (!r.ok) {
      addError = readErrorText(r.error);
      return;
    }
    addTitle = '';
    if (!addError) addingIn = null;
    void load();
  }

  // ── Multi-select (G3.5) ──
  let picked = $state.raw<Set<string>>(new Set());
  function togglePick(id: string) {
    const next = new Set(picked);
    if (!next.delete(id)) next.add(id);
    picked = next;
  }
  const pickedTasks = $derived(tasks.filter((t) => picked.has(t.task_id)));
  /** The status's own columns: where a selection can move to. */
  const statusLanes = $derived(columns.lanes.filter((l) => l.id === l.status));
  let movingAll = $state(false);
  async function movePicked(laneId: string) {
    const to = laneById(laneId);
    if (!to || movingAll) return;
    movingAll = true;
    for (const t of pickedTasks) await move(t, to);
    movingAll = false;
    // The ones refused keep their error on the card and stay selected.
    picked = new Set([...picked].filter((id) => cardErrors.has(id)));
  }

  const rowsById = $derived(new Map($sessions.map((r) => [r.id, r])));

  function open(t: WorkTask) {
    sidebarView.set('work');
    openTask(t.task_id, t.sessions);
  }

  // ── Pointer drag ──
  const DRAG_SLOP = 6;
  let press: { task: WorkTask; x: number; y: number } | null = null;
  let dragging = $state<WorkTask | null>(null);
  let ghost = $state({ x: 0, y: 0 });
  let over = $state<string | null>(null);
  /** The click a drag ends with is not an "open". */
  let swallowClick = false;

  function columnAt(x: number, y: number): BoardLane | null {
    const el = document.elementFromPoint?.(x, y)?.closest<HTMLElement>('[data-board-column]');
    return laneById(el?.dataset.boardColumn) ?? null;
  }

  function onpointerdown(e: PointerEvent, t: WorkTask) {
    if (e.button !== 0) return;
    press = { task: t, x: e.clientX, y: e.clientY };
    window.addEventListener('pointermove', onpointermove);
    window.addEventListener('pointerup', onpointerup);
    window.addEventListener('pointercancel', endDrag);
  }
  function onpointermove(e: PointerEvent) {
    if (!press) return;
    if (!dragging) {
      if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_SLOP) return;
      dragging = press.task;
    }
    ghost = { x: e.clientX, y: e.clientY };
    over = columnAt(e.clientX, e.clientY)?.id ?? null;
  }
  function onpointerup(e: PointerEvent) {
    const t = dragging;
    const to = t ? columnAt(e.clientX, e.clientY) : null;
    if (t) swallowClick = true;
    endDrag();
    if (t && to) void move(t, to);
  }
  function endDrag() {
    press = null;
    dragging = null;
    over = null;
    window.removeEventListener('pointermove', onpointermove);
    window.removeEventListener('pointerup', onpointerup);
    window.removeEventListener('pointercancel', endDrag);
  }

  function onclick(e: MouseEvent, t: WorkTask) {
    if (swallowClick) {
      swallowClick = false;
      return;
    }
    if (e.shiftKey) {
      togglePick(t.task_id);
      return;
    }
    open(t);
  }

  function oncardkey(e: KeyboardEvent, t: WorkTask, lane: BoardLane) {
    // The keys are the registry's `work-board` rows (step 0.1).
    const act = viewKey('work-board', e);
    if (act === 'work-board.edit' && !boardMoveRefusal(t) && !editBlocked) {
      e.preventDefault();
      editing = t.task_id;
      return;
    }
    if (act === 'work-board.select') {
      e.preventDefault();
      togglePick(t.task_id);
      return;
    }
    if (act === 'work-board.start') {
      e.preventDefault();
      // The open card carries the Work button: open it, then ask it.
      if ($selectedTaskId !== t.task_id) open(t);
      void tick().then(() => workButtonFor(t.task_id)?.ask());
      return;
    }
    if (act !== 'work-board.left' && act !== 'work-board.right') return;
    const to = boardStep(columns.lanes, lane, act === 'work-board.right' ? 1 : -1);
    if (!to) return;
    e.preventDefault();
    void move(t, to).then(() => {
      document.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(t.task_id)}"]`)?.focus();
    });
  }

  function onwindowkey(e: KeyboardEvent) {
    if (dragging && viewKey('work-board', e) === 'work-board.cancel-drag') {
      e.preventDefault();
      e.stopPropagation();
      endDrag();
    }
  }

  function pickScope(v: string) {
    scopeNotice = null;
    boardScope.set(v === 'all' || v === 'none' ? v : Number(v));
  }

  async function startSprint(b: BucketRow) {
    const r = await advanceBucket(b);
    scopeNotice = r.ok ? (r.value?.warning ? `Started. ${r.value.warning}.` : null) : readErrorText(r.error);
    if (r.ok) void loadSprints();
  }

  /** "12 Oct – 23 Oct". */
  function sprintDates(b: BucketRow): string {
    const d = (s: number | null | undefined) => (typeof s === 'number' ? unixToDay(s) : '');
    if (!b.starts_at && !b.ends_at) return '';
    return `${d(b.starts_at) || '…'} – ${d(b.ends_at) || '…'}`;
  }

  function badge(t: WorkTask): string {
    if (t.kind === 'local') return 'local';
    if (t.kind === 'ref') return 'key';
    return providerInfo(t.provider)?.icon ?? t.provider ?? 'tracker';
  }
</script>

<svelte:window onkeydowncapture={onwindowkey} />

<section class="board" data-testid="work-board" aria-label="Task board">
  <header>
    <!-- How to move a task is a one-time hint now (redesign 1.4), not a line on
         every visit; a reason the board cannot move tasks still shows here. -->
    <h2 use:hintAnchor={{ id: 'board-move', when: !moveBlocked }}>Board</h2>
    {#if sprints && (sprints.length > 0 || scope !== 'all')}
      <select
        class="scope"
        aria-label="Show on the board"
        data-testid="work-board-scope"
        value={String(scope)}
        onchange={(e) => pickScope((e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="all">All tasks</option>
        {#each openSprints as b (b.id)}
          <option value={String(b.id)}>{b.name}{b.state === 'active' ? ' · active' : ' · planned'}</option>
        {/each}
        <option value="none">No sprint (backlog)</option>
      </select>
    {/if}
    {#if moveBlocked}<span class="muted" data-testid="work-board-hint">{moveBlocked}</span>{/if}
    {#if onclose}
      <button class="btn btn--quiet btn--icon" type="button" title="Close the board" aria-label="Close the board"
        data-testid="work-board-close" onclick={() => onclose?.()}>✕</button
      >
    {/if}
  </header>

  {#if sprint}
    <div class="sprint" data-testid="work-board-sprint">
      <span class="sprint-sum">{bucketSummary(sprint)}</span>
      {#if sprintDates(sprint)}<span class="muted">{sprintDates(sprint)}</span>{/if}
      {#if sprint.goal}<span class="muted goal" title="Sprint goal">{sprint.goal}</span>{/if}
      <span class="sprint-actions">
        {#if sprint.state === 'planned'}
          <button
            class="btn btn--quiet"
            type="button"
            data-testid="work-board-sprint-start"
            disabled={adminBlocked !== null}
            title={adminBlocked ?? 'Start this sprint'}
            onclick={() => void startSprint(sprint)}>Start sprint</button
          >
        {/if}
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="work-board-sprint-close"
          disabled={adminBlocked !== null}
          title={adminBlocked ?? 'Close it, and choose what carries over'}
          onclick={() => (closingSprint = sprint.id)}>Close sprint…</button
        >
      </span>
    </div>
  {/if}
  {#if scopeNotice}<p class="muted pad" role="status" data-testid="work-board-scope-notice">{scopeNotice}</p>{/if}
  {#if picked.size > 0}
    <div class="pickbar" role="toolbar" aria-label="Selected cards" data-testid="work-board-pickbar">
      <span>{picked.size} selected</span>
      <label
        >Move to
        <select
          data-testid="work-board-pick-move"
          disabled={movingAll || moveBlocked !== null}
          value=""
          onchange={(e) => void movePicked((e.currentTarget as HTMLSelectElement).value)}
        >
          <option value="" disabled>column…</option>
          {#each statusLanes as l (l.id)}<option value={l.id}>{l.label}</option>{/each}
        </select></label
      >
      <button class="btn btn--quiet" type="button" data-testid="work-board-pick-clear" onclick={() => (picked = new Set())}>Clear</button>
    </div>
  {/if}
  {#if error}
    <p class="err" role="alert" data-testid="work-board-error">
      {readErrorText(error)}
      <button class="btn btn--quiet" type="button" onclick={() => void load()}>Retry</button>
    </p>
  {:else if !loaded}
    <div class="pad" data-testid="work-board-loading"><Skeleton rows={4} label="Loading tasks" /></div>
  {:else}
    {#if more}
      <p class="muted pad" data-testid="work-board-more">Showing the 200 most recent tasks. Narrow the Work view's filters to see the rest.</p>
    {/if}
    {@const facets = workFacets({ ...$workViewFilters, status: undefined })}
    {#if facets.length > 0 && columns.lanes.every((l) => (columns.cards[l.id] ?? []).length === 0)}
      <!-- Review r13 (step 10.6): filters that hide every task say so and
           offer the way out, as the list does. -->
      <p class="muted pad" data-testid="work-board-no-match">
        No tasks match <strong>{facetSentence(facets)}</strong>.
        <button
          class="btn btn--quiet"
          type="button"
          data-testid="work-board-clear"
          onclick={() => {
            activeWorkViewId.set(null);
            workViewFilters.set({});
          }}>Clear filters</button
        >
      </p>
    {/if}
    <div class="columns" style:grid-template-columns="repeat({columns.lanes.length}, minmax(200px, 1fr))">
      {#each columns.lanes as lane (lane.id)}
        {@const nodes = columns.cards[lane.id] ?? []}
        <div
          class="column"
          class:over={over === lane.id && dragging && laneOfCard(dragging).id !== lane.id}
          data-board-column={lane.id}
          data-status={lane.status}
          data-testid="work-board-column-{lane.id}"
          role="group"
          aria-label={lane.label}
        >
          <h3>
            {lane.label}{#if lane.id === 'done' && typeof scope !== 'number'}<span class="window"> · last 7 days</span>{/if}
            <span class="count">{nodes.length}</span>
            {#if lane.id === lane.status}
              <button
                class="add"
                type="button"
                title="Add a task to {lane.label}"
                aria-label="Add a task to {lane.label}"
                data-testid="work-board-add-{lane.id}"
                onclick={() => {
                  addingIn = addingIn === lane.id ? null : lane.id;
                  addError = null;
                }}>+</button
              >
            {/if}
          </h3>
          {#if addingIn === lane.id}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="add-input"
              placeholder="New task title"
              aria-label="New task in {lane.label}"
              data-testid="work-board-add-input"
              autofocus
              disabled={addBusy}
              bind:value={addTitle}
              onkeydown={(e) => {
                if (e.key === 'Enter') void addIn(lane);
                if (e.key === 'Escape') addingIn = null;
              }}
            />
            {#if addError}<p class="card-err" role="alert" data-testid="work-board-add-error">{addError}</p>{/if}
          {/if}
          {#if over === lane.id && dragging && laneOfCard(dragging).id !== lane.id}
            <p class="drop" data-testid="work-board-drop">Drop to move to {lane.label}</p>
          {/if}
          <ul>
            {#each nodes as n (n.task.task_id)}
              {@render card(n, lane)}
            {/each}
          </ul>
          {#if nodes.length === 0}<p class="muted empty">Nothing here.</p>{/if}
          {#if lane.id === 'done' && columns.doneHidden > 0}
            <p class="muted empty" data-testid="work-board-done-hidden">{columns.doneHidden} older not shown</p>
          {/if}
        </div>
      {/each}
    </div>
    {#if trackerLanes}
      <p class="muted board-note" data-testid="work-board-note">
        Columns come from the tracker. List and Board place a task in the same status.
      </p>
    {/if}
  {/if}
</section>

{#if dragging}
  <div class="ghost" style="left: {ghost.x + 8}px; top: {ghost.y + 8}px" aria-hidden="true">
    {displayTitle(dragging)}
  </div>
{/if}

{#if closingSprint != null}
  <!-- Its close bumps `workChanged`, which re-reads the board. -->
  <WorkBuckets closeId={closingSprint} onclose={() => (closingSprint = null)} />
{/if}

{#if editing}
  <EditTaskDialog taskId={editing} onclose={() => (editing = null)} ondone={() => void load()} />
{/if}

{#snippet card(n: TaskNode, lane: BoardLane)}
  {@const t = n.task}
  {@const refusal = boardMoveRefusal(t)}
  {@const live = boardLiveSession(t)}
  {@const err = cardErrors.get(t.task_id)}
  {@const owner = ownerDueChip(t)}
  {@const pr = prChip(t, rowsById)}
  {@const liveRow = live ? rowsById.get(live.session_id) : undefined}
  <li role="listitem">
    <button
      class="card"
      class:selected={$selectedTaskId === t.task_id}
      class:dragging={dragging?.task_id === t.task_id}
      class:locked={!!refusal}
      class:picked={picked.has(t.task_id)}
      aria-pressed={picked.has(t.task_id) ? 'true' : undefined}
      type="button"
      data-board-card={t.task_id}
      data-testid="work-board-card"
      title={refusal ?? undefined}
      aria-current={$selectedTaskId === t.task_id ? 'true' : undefined}
      onpointerdown={(e) => onpointerdown(e, t)}
      onclick={(e) => onclick(e, t)}
      onkeydown={(e) => oncardkey(e, t, lane)}
    >
      <span class="top">
        <span class="tb" title={t.tracker_name ?? t.kind}>{badge(t)}</span>
        {#if t.key}<span class="key">{t.key}</span>{/if}
        {#if refusal}<span class="lock" role="img" aria-label="status set elsewhere"><Icon name="lock" size={12} /></span>{/if}
        {#if t.needs_you}<span class="needs" title="A session needs you" aria-label="needs you">●</span>{/if}
      </span>
      <span class="title" class:derived={t.title_derived}>{displayTitle(t)}</span>
      <span class="meta">
        {#if t.status_name}<span>{t.status_name}</span>{/if}
        {#if t.project_label}<span>{t.project_label}</span>{/if}
        {#if t.epic}<span class="epic" data-testid="work-board-epic">Epic</span>{/if}
        {#if (t.children_total ?? 0) > 0}<span data-testid="work-board-rollup">{t.children_done ?? 0}/{t.children_total} done</span>
        {:else if n.children.length > 0}<span>{n.children.length} subtask{n.children.length === 1 ? '' : 's'}</span>{/if}
        {#if (t.open_proposals ?? 0) > 0}<span class="prop">{t.open_proposals} to review</span>{/if}
        <TaskBlockedSpend task={t} lookup={taskById} testid="work-board-card" />
      </span>
      {#if owner}
        <span
          class="owner"
          class:overdue={owner.overdue}
          title={owner.overdue ? `Overdue since ${t.due_at}` : t.due_at ? `Due ${t.due_at}` : undefined}
          data-testid="work-board-owner">{owner.text}</span
        >
      {/if}
      {#if pr}
        <span class="pr" data-testid="work-board-pr" title={pr.url}
          >{pr.label}{#if pr.checks === 'passing'}<span class="ok" aria-label="checks passing">{' ✓'}</span>{:else if pr.checks === 'failing'}<span
              class="bad"
              aria-label="{pr.failing} failing">{` ✕ ${pr.failing}`}</span
            >{:else if pr.checks === 'running'}<span class="muted" aria-label="checks running">{' …'}</span>{/if}</span
        >
      {/if}
      {#if live}
        <span class="live" data-testid="work-board-live"
          >● {live.host ? `${live.name} · ${live.host}` : live.name}{#if liveRow}<span class="agent" data-testid="work-board-agent"
              >{` · ${AGENT_LABELS[sessionAgent(liveRow)]}`}</span
            >{/if}</span
        >
      {/if}
    </button>
    {#if !refusal}
      <button
        class="edit"
        type="button"
        title={editBlocked ?? 'Edit task (E)'}
        aria-label="Edit {displayTitle(t)}"
        disabled={editBlocked !== null}
        data-testid="work-board-card-edit"
        onclick={() => (editing = t.task_id)}><Icon name="edit" size={12} /></button
      >
    {/if}
    {#if err}<p class="card-err" role="alert" data-testid="work-board-card-error">{err}</p>{/if}
    {#if $selectedTaskId === t.task_id}
      <div class="card-work" data-testid="work-board-card-work"><WorkButton task={t} /></div>
    {/if}
  </li>
{/snippet}

<style>
  .scope {
    font: inherit;
    font-size: var(--text-xs);
    max-width: 16rem;
  }
  .sprint {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .sprint-sum {
    font-weight: 500;
  }
  .goal {
    font-style: italic;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sprint-actions {
    margin-left: auto;
    display: flex;
    gap: var(--space-1);
  }
  .epic {
    color: var(--accent);
    font-weight: 500;
  }
  .board-note {
    margin: var(--space-2) 0 0;
    font-size: var(--text-2xs);
  }
  .board {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
    font-size: var(--text-xs);
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0;
    font-size: var(--text-sm);
  }
  /* The close button sits at the right edge whether or not a reason shows. */
  header [data-testid='work-board-close'] {
    margin-left: auto;
  }
  header .muted {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--fg-muted);
  }
  .pad {
    padding: var(--space-2) var(--space-3);
    margin: 0;
  }
  .err {
    padding: var(--space-2) var(--space-3);
    color: var(--usage-crit);
  }
  .columns {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: repeat(3, minmax(200px, 1fr));
    gap: 10px;
    padding: 10px var(--space-3);
    overflow: auto;
  }
  .column {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--fg) 3%, transparent);
    overflow: auto;
  }
  .column.over {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  h3 {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0;
    padding: var(--space-2) 10px var(--space-1);
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    font-weight: 600;
  }
  .window {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
  }
  .count {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: var(--space-1) var(--space-2) var(--space-2);
    display: grid;
    gap: 6px;
  }
  .empty {
    margin: 0;
    padding: 0 10px 10px;
    font-size: var(--text-2xs);
  }
  .card {
    width: 100%;
    display: grid;
    gap: 3px;
    padding: 6px var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .card.locked {
    cursor: pointer;
  }
  li {
    position: relative;
  }
  .edit {
    position: absolute;
    top: 4px;
    right: 4px;
    padding: 0 5px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-2xs);
    cursor: pointer;
    opacity: 0;
  }
  li:hover .edit,
  .edit:focus-visible {
    opacity: 1;
  }
  .edit:hover {
    color: var(--fg);
  }
  .edit:disabled {
    cursor: not-allowed;
  }
  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .card.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--bg));
  }
  .card.dragging {
    opacity: 0.4;
  }
  .card:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .top {
    display: flex;
    gap: 5px;
    align-items: center;
  }
  .tb {
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
    padding: 0 0.2rem;
    color: var(--fg-muted);
  }
  .key {
    font-family: var(--mono);
    font-size: var(--text-2xs);
  }
  .lock {
    font-size: var(--text-2xs);
  }
  .needs {
    color: var(--usage-crit);
    font-size: var(--text-2xs);
    margin-left: auto;
  }
  .title {
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .derived {
    font-style: italic;
    color: var(--fg-muted);
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-2);
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .meta > span {
    white-space: nowrap;
  }
  .prop {
    color: var(--accent);
  }
  .owner {
    align-self: flex-start;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    background: var(--chip-bg, color-mix(in srgb, currentColor 10%, transparent));
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    white-space: nowrap;
  }
  .owner.overdue {
    background: var(--failed-soft);
    color: var(--status-failed);
  }
  .live {
    font-size: var(--text-2xs);
    color: var(--usage-ok);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card.picked {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .pr {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .ok {
    color: var(--status-done);
  }
  .bad {
    color: var(--status-failed);
  }
  .add {
    margin-left: auto;
    border: 0;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    cursor: pointer;
  }
  .add:hover {
    color: var(--fg);
  }
  .add-input {
    width: 100%;
    margin-bottom: 4px;
    font: inherit;
  }
  .drop {
    margin: 0 0 4px;
    padding: 4px;
    border: 1px dashed var(--accent);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    text-align: center;
  }
  .pickbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
  }
  .card-work {
    padding: 4px 0 0;
  }
  .card-err {
    margin: 2px 2px 0;
    font-size: var(--text-2xs);
    color: var(--usage-crit);
  }
  .ghost {
    position: fixed;
    z-index: 1000;
    pointer-events: none;
    max-width: 240px;
    padding: var(--space-1) var(--space-2);
    border: 1px solid var(--accent);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font-size: var(--text-2xs);
    box-shadow: var(--shadow-pop);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @media (max-width: 720px) {
    .columns {
      grid-template-columns: repeat(3, minmax(180px, 1fr));
    }
  }
</style>
