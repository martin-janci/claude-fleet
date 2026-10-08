<script lang="ts">
  // The task board (sprints design 2026-09-28 §6c): every task the Work
  // view's filters match, in To do / Doing / Done columns, from one
  // `work_tree` read (archived on, so Done has its rows; the status filter
  // off, since the columns are the status). A native task's card drags to
  // another column — the one place a status is set by dragging, a person's
  // setting that is final over the derived one. A tracker's card sits in
  // the column its tracker reports and says so when dragged (E11), rather
  // than failing silently. A card shows its live session and host.
  //
  // The drag is pointer events, not HTML5 drag and drop: the window takes
  // OS file drops (`dragDropEnabled`), which on Windows swallows the
  // webview's own. ← / → on a focused card move it too. Tracker and agent
  // text renders as text. A native card's ✎ (or E on the focused card)
  // opens the edit dialog: title, description, status, assignees.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { onWorkChangedDebounced, setWorkStatus } from './work';
  import {
    openTask,
    readErrorText,
    selectedTaskId,
    sidebarView,
    workTree,
    workViewFilters,
    type WorkTask,
  } from './work_view';
  import { providerInfo } from './trackers';
  import { hintAnchor } from './hints';
  import EditTaskDialog from './EditTaskDialog.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    BOARD_COLUMNS,
    BOARD_COLUMN_LABELS,
    BOARD_COLUMN_STATUS,
    boardColumnOf,
    boardLiveSession,
    boardMoveRefusal,
    displayTitle,
    groupTasksForBoard,
    type BoardColumn,
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
  let more = $state(false);
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  /** Cards a person moved, placed before the hub answers. */
  let overrides = $state.raw<Map<string, BoardColumn>>(new Map());
  /** Cards moved on this board: kept on Done past its window. */
  let moved = $state.raw<Set<string>>(new Set());
  /** A refused or failed move, on the card it was about. */
  let cardErrors = $state.raw<Map<string, string>>(new Map());

  /** The task whose edit dialog is open. */
  let editing = $state<string | null>(null);
  const editBlocked = $derived(hubActionBlocked('edit_work_item', $hubStatus, $hubConnection));
  const moveBlocked = $derived(hubActionBlocked('set_work_status', $hubStatus, $hubConnection));
  const columns = $derived(groupTasksForBoard(tasks, Math.floor(Date.now() / 1000), overrides, moved));

  let seq = 0;
  async function load() {
    const mine = ++seq;
    const { status: _s, ...filters } = get(workViewFilters);
    const r = await workTree({ filters: { ...filters, archived: true }, limit: 200, per_task: 3 });
    if (mine !== seq) return;
    loaded = true;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
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
  const offChanged = onWorkChangedDebounced(
    () => void load(),
    () => debounceMs,
    () => maxWaitMs,
  );
  onMount(() => void load());
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
   *  the status of, else placed at once and set on the hub. */
  async function move(t: WorkTask, to: BoardColumn) {
    const from = overrides.get(t.task_id) ?? boardColumnOf(t);
    if (from === to) return;
    const refusal = boardMoveRefusal(t);
    if (refusal) {
      setCardError(t.task_id, refusal);
      return;
    }
    if (moveBlocked) {
      setCardError(t.task_id, moveBlocked);
      return;
    }
    setCardError(t.task_id, null);
    overrides = new Map(overrides).set(t.task_id, to);
    moved = new Set(moved).add(t.task_id);
    const r = await setWorkStatus(t.item_id as number, BOARD_COLUMN_STATUS[to]);
    if (!r.ok) {
      const back = new Map(overrides);
      back.delete(t.task_id);
      overrides = back;
      setCardError(t.task_id, readErrorText(r.error));
    }
  }

  function open(t: WorkTask) {
    sidebarView.set('work');
    openTask(t.task_id);
  }

  // ── Pointer drag ──
  const DRAG_SLOP = 6;
  let press: { task: WorkTask; x: number; y: number } | null = null;
  let dragging = $state<WorkTask | null>(null);
  let ghost = $state({ x: 0, y: 0 });
  let over = $state<BoardColumn | null>(null);
  /** The click a drag ends with is not an "open". */
  let swallowClick = false;

  function columnAt(x: number, y: number): BoardColumn | null {
    const el = document.elementFromPoint?.(x, y)?.closest<HTMLElement>('[data-board-column]');
    const c = el?.dataset.boardColumn;
    return c && (BOARD_COLUMNS as readonly string[]).includes(c) ? (c as BoardColumn) : null;
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
    over = columnAt(e.clientX, e.clientY);
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

  function onclick(t: WorkTask) {
    if (swallowClick) {
      swallowClick = false;
      return;
    }
    open(t);
  }

  function oncardkey(e: KeyboardEvent, t: WorkTask, col: BoardColumn) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if ((e.key === 'e' || e.key === 'E') && !boardMoveRefusal(t) && !editBlocked) {
      e.preventDefault();
      editing = t.task_id;
      return;
    }
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    const i = BOARD_COLUMNS.indexOf(col) + (e.key === 'ArrowRight' ? 1 : -1);
    if (i < 0 || i >= BOARD_COLUMNS.length) return;
    e.preventDefault();
    void move(t, BOARD_COLUMNS[i]).then(() => {
      document.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(t.task_id)}"]`)?.focus();
    });
  }

  function onwindowkey(e: KeyboardEvent) {
    if (e.key === 'Escape' && dragging) {
      e.preventDefault();
      e.stopPropagation();
      endDrag();
    }
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
    {#if moveBlocked}<span class="muted" data-testid="work-board-hint">{moveBlocked}</span>{/if}
    <button class="btn btn--quiet btn--icon" type="button" title="Close the board" aria-label="Close the board"
      data-testid="work-board-close" onclick={() => onclose?.()}>✕</button
    >
  </header>

  {#if error}
    <p class="err" role="alert" data-testid="work-board-error">
      {readErrorText(error)}
      <button class="btn btn--quiet" type="button" onclick={() => void load()}>Retry</button>
    </p>
  {:else if !loaded}
    <p class="muted pad" data-testid="work-board-loading">Loading tasks…</p>
  {:else}
    {#if more}
      <p class="muted pad" data-testid="work-board-more">Showing the 200 most recent tasks. Narrow the Work view's filters to see the rest.</p>
    {/if}
    <div class="columns">
      {#each BOARD_COLUMNS as col (col)}
        {@const nodes = columns[col]}
        <div
          class="column"
          class:over={over === col && dragging && (overrides.get(dragging.task_id) ?? boardColumnOf(dragging)) !== col}
          data-board-column={col}
          data-testid="work-board-column-{col}"
          role="list"
          aria-label={BOARD_COLUMN_LABELS[col]}
        >
          <h3>
            {BOARD_COLUMN_LABELS[col]}{#if col === 'done'}<span class="window"> · last 7 days</span>{/if}
            <span class="count">{nodes.length}</span>
          </h3>
          <ul>
            {#each nodes as n (n.task.task_id)}
              {@render card(n, col)}
            {/each}
          </ul>
          {#if nodes.length === 0}<p class="muted empty">Nothing here.</p>{/if}
          {#if col === 'done' && columns.doneHidden > 0}
            <p class="muted empty" data-testid="work-board-done-hidden">{columns.doneHidden} older not shown</p>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</section>

{#if dragging}
  <div class="ghost" style="left: {ghost.x + 8}px; top: {ghost.y + 8}px" aria-hidden="true">
    {displayTitle(dragging)}
  </div>
{/if}

{#if editing}
  <EditTaskDialog taskId={editing} onclose={() => (editing = null)} ondone={() => void load()} />
{/if}

{#snippet card(n: TaskNode, col: BoardColumn)}
  {@const t = n.task}
  {@const refusal = boardMoveRefusal(t)}
  {@const live = boardLiveSession(t)}
  {@const err = cardErrors.get(t.task_id)}
  <li role="listitem">
    <button
      class="card"
      class:selected={$selectedTaskId === t.task_id}
      class:dragging={dragging?.task_id === t.task_id}
      class:locked={!!refusal}
      type="button"
      data-board-card={t.task_id}
      data-testid="work-board-card"
      title={refusal ?? undefined}
      aria-current={$selectedTaskId === t.task_id ? 'true' : undefined}
      onpointerdown={(e) => onpointerdown(e, t)}
      onclick={() => onclick(t)}
      onkeydown={(e) => oncardkey(e, t, col)}
    >
      <span class="top">
        <span class="tb" title={t.tracker_name ?? t.kind}>{badge(t)}</span>
        {#if t.key}<span class="key">{t.key}</span>{/if}
        {#if refusal}<span class="lock" aria-label="status set elsewhere">🔒</span>{/if}
        {#if t.needs_you}<span class="needs" title="A session needs you" aria-label="needs you">●</span>{/if}
      </span>
      <span class="title" class:derived={t.title_derived}>{displayTitle(t)}</span>
      <span class="meta">
        {#if t.status_name}<span>{t.status_name}</span>{/if}
        {#if t.project_label}<span>{t.project_label}</span>{/if}
        {#if n.children.length > 0}<span>{n.children.length} subtask{n.children.length === 1 ? '' : 's'}</span>{/if}
        {#if (t.open_proposals ?? 0) > 0}<span class="prop">{t.open_proposals} to review</span>{/if}
      </span>
      {#if live}
        <span class="live" data-testid="work-board-live">● {live.host ? `${live.name} · ${live.host}` : live.name}</span>
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
        onclick={() => (editing = t.task_id)}>✎</button
      >
    {/if}
    {#if err}<p class="card-err" role="alert" data-testid="work-board-card-error">{err}</p>{/if}
  </li>
{/snippet}

<style>
  .board {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--fg);
    font-size: 0.85rem;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0;
    font-size: 0.95rem;
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
    padding: 8px 12px;
    margin: 0;
  }
  .err {
    padding: 8px 12px;
    color: var(--usage-crit, #c62828);
  }
  .columns {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: repeat(3, minmax(200px, 1fr));
    gap: 10px;
    padding: 10px 12px;
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
    padding: 8px 10px 4px;
    font-size: 0.7rem;
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
    padding: 4px 8px 8px;
    display: grid;
    gap: 6px;
  }
  .empty {
    margin: 0;
    padding: 0 10px 10px;
    font-size: 0.75rem;
  }
  .card {
    width: 100%;
    display: grid;
    gap: 3px;
    padding: 6px 8px;
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
    font-size: 0.75rem;
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
    font-size: 0.62rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.2rem;
    color: var(--fg-muted);
  }
  .key {
    font-family: var(--mono);
    font-size: 0.75rem;
  }
  .lock {
    font-size: 0.65rem;
  }
  .needs {
    color: var(--usage-crit, #c62828);
    font-size: 0.55rem;
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
    gap: 0 8px;
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .meta > span {
    white-space: nowrap;
  }
  .prop {
    color: var(--accent);
  }
  .live {
    font-size: 0.72rem;
    color: var(--usage-ok);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-err {
    margin: 2px 2px 0;
    font-size: 0.72rem;
    color: var(--usage-crit, #c62828);
  }
  .ghost {
    position: fixed;
    z-index: 1000;
    pointer-events: none;
    max-width: 240px;
    padding: 4px 8px;
    border: 1px solid var(--accent);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font-size: 0.8rem;
    box-shadow: 0 4px 12px rgb(0 0 0 / 0.2);
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
