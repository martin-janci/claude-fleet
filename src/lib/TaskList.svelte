<script lang="ts">
  import Icon from './kit/Icon.svelte';
  // The Work tab's List layout (design 2026-09-29): every task the current
  // filters match — tickets, native tasks, bare keys — in To do / Doing /
  // Done, from ONE `work_tree` read (archived on, so Done has its rows).
  // The header (Tasks | Review, List | Grouped, saved views, filters) is
  // WorkTree's; this is only the body. Proposals show here only as the
  // "N to review" badge: they are decided on the task page. Tracker and
  // agent text renders as text.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { viewKey } from './shortcuts';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import {
    openTask,
    readErrorText,
    selectedTaskId,
    workTree,
    workViewFilters,
    type WorkTask,
    type WorkTaskLink,
    type WorkTreePage,
  } from './work_view';
  import WorkTaskRow from './WorkTaskRow.svelte';
  import { sessions } from './sessions';
  import { selectedSession, selectSessionExplicitly } from './selection';
  import { workButtonFor } from './start_preview';
  import WorkButton from './WorkButton.svelte';
  import EditTaskDialog from './EditTaskDialog.svelte';
  import NewTaskDialog from './NewTaskDialog.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { displayTitle, groupTasksByStatus, type StatusSections, type TaskNode } from './task_list';
  import type { IpcError } from './result';

  let {
    /** The refetch debounce, ms; injectable for tests. */
    debounceMs = 500,
    /** The longest a steady stream of changes may hold a refetch back, ms. */
    maxWaitMs = 3000,
    /** Each page read, so the header can name its orgs and trackers. */
    onpage,
  }: { debounceMs?: number; maxWaitMs?: number; onpage?: (p: WorkTreePage) => void } = $props();

  let tasks = $state.raw<WorkTask[]>([]);
  // A blocked task names what it waits for by key when that task is loaded.
  const byId = $derived(new Map(tasks.map((t) => [t.task_id, t])));
  const taskById = (id: string) => byId.get(id);
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  let doneOpen = $state(false);
  let addTitle = $state('');
  /** The New task dialog is open (▾; G2.1), seeded with `addTitle`. */
  let addMore = $state(false);
  let busy = $state(false);
  let actionError = $state<string | null>(null);
  /** The task whose edit dialog is open. */
  let editing = $state<string | null>(null);
  const editBlocked = $derived(hubActionBlocked('edit_work_item', $hubStatus, $hubConnection));

  const sections: StatusSections = $derived(groupTasksByStatus(tasks, Math.floor(Date.now() / 1000)));
  const emptySections = $derived(
    [
      sections.todo.length === 0 ? 'To do' : null,
      sections.doing.length === 0 ? 'Doing' : null,
      sections.done.length === 0 ? 'Done in the last 7 days' : null,
    ].filter((x): x is string => x !== null),
  );

  let seq = 0;
  async function load() {
    const mine = ++seq;
    const r = await workTree({ filters: { ...get(workViewFilters), archived: true }, limit: 200, per_task: 3 });
    if (mine !== seq) return;
    loaded = true;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
    const orgs = Array.isArray(r.value?.orgs) ? r.value.orgs : [];
    onpage?.({
      ...r.value,
      tasks,
      orgs,
      groups: Array.isArray(r.value?.groups) ? r.value.groups : [],
      trackers: Array.isArray(r.value?.trackers) ? r.value.trackers : [],
    });
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
  onMount(() => {
    void load();
  });
  onDestroy(() => {
    offFilters();
    offChanged();
  });

  async function add() {
    const title = addTitle.trim();
    if (!title || busy) return;
    busy = true;
    const r = await createWorkTask({ title });
    busy = false;
    if (!r.ok) {
      actionError = readErrorText(r.error);
      return;
    }
    actionError = null;
    addTitle = '';
    void load();
  }

  /** The task rows in the order they show: To do, Doing, then Done when
   *  it is open. */
  const visibleRows = $derived(
    [...sections.todo, ...sections.doing, ...(doneOpen ? sections.done : [])].map((n) => n.task.task_id),
  );

  const nodeOf = (id: string) => [...sections.todo, ...sections.doing, ...sections.done].find((n) => n.task.task_id === id);

  /** The list's keyboard (task → session spec §2.2): `j` / `k` move the
   *  selection, `s` runs the selected task's Work button, ⇧S opens its
   *  start popover. Never inside a field, a menu or a dialog. */
  function onkey(e: KeyboardEvent) {
    // The keys are the registry's `task-list` rows (step 0.1).
    const act = viewKey('task-list', e);
    if (!act) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest('input, textarea, select, [role="dialog"], [role="menu"]')) return;
    const at = visibleRows.indexOf(get(selectedTaskId) ?? '');
    if (act === 'task-list.down' || act === 'task-list.up') {
      if (visibleRows.length === 0) return;
      e.preventDefault();
      const next = act === 'task-list.down' ? Math.min(at + 1, visibleRows.length - 1) : Math.max(at - 1, 0);
      const id = visibleRows[at < 0 ? 0 : next];
      openTask(id, nodeOf(id)?.task.sessions);
      document.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(id)}"] .main`)?.focus();
    } else if (at >= 0) {
      const b = workButtonFor(visibleRows[at]);
      if (!b) return;
      e.preventDefault();
      if (act === 'task-list.work-ask') b.ask();
      else b.primary();
    }
  }
  // A session chip opens that session, as in the Grouped tree.
  function openLink(t: WorkTask, l: WorkTaskLink) {
    const row = l.session_id != null ? $sessions.find((r) => r.id === l.session_id) : undefined;
    if (row) selectSessionExplicitly(row, { task: t.task_id });
    else openTask(t.task_id, t.sessions);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="task-list" data-testid="task-list" onkeydown={onkey}>
  <div class="add">
    <input
      placeholder="+ New task"
      aria-label="New task"
      data-testid="task-add-input"
      bind:value={addTitle}
      onkeydown={(e) => {
        if (e.key === 'Enter') void add();
      }}
    />
    <button
      class="btn btn--quiet btn--icon"
      type="button"
      title="New task with a project, notes or a start"
      aria-label="New task with a project, notes or a start"
      data-testid="task-add-more"
      onclick={() => (addMore = true)}>▾</button
    >
  </div>
  {#if actionError}<p class="err" role="alert" data-testid="task-list-action-error">{actionError}</p>{/if}

  {#if error}
    <p class="err" role="alert" data-testid="task-list-error">
      {readErrorText(error)}
      <button class="btn btn--quiet" type="button" onclick={() => void load()}>Retry</button>
    </p>
  {:else if !loaded}
    <p class="muted" data-testid="task-list-loading">Loading tasks…</p>
  {:else if tasks.length === 0}
    <p class="muted" data-testid="task-list-empty">No tasks match. Type one above and press Enter.</p>
  {:else}
    <!-- An empty section is a word in one line, not a heading with nothing
         under it (redesign 1.4). -->
    {#if sections.todo.length > 0}{@render section('todo', 'To do', sections.todo, true)}{/if}
    {#if sections.doing.length > 0}{@render section('doing', 'Doing', sections.doing, true)}{/if}
    {#if sections.done.length > 0}{@render section('done', 'Done · last 7 days', sections.done, doneOpen)}{/if}
    {#if emptySections.length > 0}
      <p class="muted" data-testid="task-sections-empty">Nothing in {emptySections.join(' or ')}.</p>
    {/if}
  {/if}
</div>

{#if addMore}
  <NewTaskDialog
    initialTitle={addTitle}
    onclose={() => (addMore = false)}
    ondone={() => {
      addTitle = '';
      void load();
    }}
  />
{/if}
{#if editing}
  <EditTaskDialog taskId={editing} onclose={() => (editing = null)} ondone={() => void load()} />
{/if}

{#snippet section(id: 'todo' | 'doing' | 'done', label: string, nodes: TaskNode[], open: boolean)}
  <section data-testid="task-section-{id}">
    <h3>
      {#if id === 'done'}
        <button class="sec" type="button" aria-expanded={open} onclick={() => (doneOpen = !doneOpen)}
          >{open ? '▾' : '▸'} {label}</button
        >
      {:else}{label}{/if}
      <span class="count">{nodes.length}</span>
    </h3>
    {#if open}
      <ul>
        {#each nodes as n (n.task.task_id)}
          {@const t = n.task}
          <li class:selected={$selectedTaskId === t.task_id} data-task-id={t.task_id}>
            <div data-testid="task-row">
              <WorkTaskRow
                task={t}
                selected={$selectedTaskId === t.task_id}
                currentSessionId={$selectedSession?.id ?? null}
                lookup={taskById}
                title={displayTitle(t)}
                onselect={() => openTask(t.task_id, t.sessions)}
                onopen={(l) => openLink(t, l)}
              >
                {#snippet trailing()}
                  {#if t.kind === 'local' && t.item_id != null}
                    <button
                      class="edit"
                      type="button"
                      title={editBlocked ?? 'Edit task'}
                      aria-label="Edit {displayTitle(t)}"
                      disabled={editBlocked !== null}
                      data-testid="task-edit"
                      onclick={() => (editing = t.task_id)}><Icon name="edit" size={12} /></button
                    >
                  {/if}
                  <WorkButton task={t} />
                {/snippet}
              </WorkTaskRow>
            </div>
            {#if n.children.length > 0}
              <ul class="children">
                {#each n.children as c (c.task_id)}
                  <li class="child" data-testid="task-child">
                    <span class="dot dot--{c.status_category ?? 'todo'}" aria-hidden="true"></span>
                    <button class="txt" type="button" onclick={() => openTask(c.task_id, c.sessions)}>{displayTitle(c)}</button>
                    {#if c.origin === 'agent'}<span class="chip agent" title="A delegated job">agent</span>{:else if c.key}<span class="key">{c.key}</span>{/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/snippet}

<style>
  .task-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--text-xs);
  }
  .add {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 6px 12px 2px;
  }
  .add input {
    flex: 1;
    min-width: 0;
    height: 26px;
    padding: 0 var(--space-2);
    border: 1px dashed var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font: inherit;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    padding: 12px 12px 4px;
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    color: var(--fg-muted);
  }
  .sec {
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    text-transform: inherit;
    letter-spacing: inherit;
    cursor: pointer;
    padding: 0;
  }
  .count {
    font-size: var(--text-2xs);
    font-weight: 500;
    padding: 0 5px;
    border-radius: var(--radius-sm);
    background: var(--count-bg);
    color: var(--fg-2);
    line-height: 16px;
    font-variant-numeric: tabular-nums;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .child .txt:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .txt {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .key {
    font-family: var(--mono);
    flex: none;
  }
  /* Each fact wraps as a whole in a narrow sidebar, never mid-phrase. */
  .edit {
    background: none;
    border: 0;
    padding: 0 3px;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-2xs);
    cursor: pointer;
  }
  .edit:hover {
    color: var(--fg);
  }
  .edit:disabled {
    cursor: not-allowed;
  }
  .chip {
    font-size: var(--text-2xs);
    border-radius: var(--radius-pill);
    padding: 0 6px;
    white-space: nowrap;
  }
  .agent {
    background: var(--chip-bg);
    color: var(--fg-2);
  }
  .children {
    margin: 0 0 var(--space-1) var(--space-6);
    border-left: 1px solid var(--border);
    padding-left: var(--space-2);
  }
  .child {
    display: grid;
    grid-template-columns: 10px minmax(0, 1fr) auto;
    gap: 6px;
    align-items: center;
    font-size: var(--text-2xs);
  }
  .child .txt {
    background: none;
    border: 0;
    padding: 0;
    text-align: left;
    color: var(--fg);
    font: inherit;
    cursor: pointer;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    border: 1.5px solid var(--fg-muted);
  }
  .dot--in_progress {
    background: var(--accent);
    border-color: var(--accent);
  }
  .dot--done {
    background: var(--usage-ok);
    border-color: var(--usage-ok);
  }
  .muted {
    color: var(--fg-muted);
    padding: 6px 12px;
    margin: 0;
  }
  .err {
    color: var(--usage-crit);
    padding: var(--space-1);
    margin: 0;
  }
</style>
