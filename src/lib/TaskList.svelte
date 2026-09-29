<script lang="ts">
  // The Work tab's List layout (design 2026-09-29): every task the current
  // filters match — tickets, native tasks, bare keys — in To do / Doing /
  // Done, from ONE `work_tree` read (archived on, so Done has its rows).
  // The header (Tasks | Review, List | Grouped, saved views, filters) is
  // WorkTree's; this is only the body. Proposals show here only as the
  // "N to review" badge: they are decided on the task page. Tracker and
  // agent text renders as text.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import {
    openTask,
    readErrorText,
    selectedTaskId,
    workTree,
    workViewFilters,
    type WorkTask,
    type WorkTreePage,
  } from './work_view';
  import { providerInfo, startWork } from './trackers';
  import { projects, loadProjects } from './projects';
  import { selectSessionExplicitly } from './selection';
  import { hubStatus, hubActionBlocked } from './hub';
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

  const startBlocked = $derived(hubActionBlocked('start_work', $hubStatus, $hubConnection));

  let tasks = $state.raw<WorkTask[]>([]);
  let orgNames = $state.raw<Map<number, string>>(new Map());
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  let doneOpen = $state(false);
  let addTitle = $state('');
  let addMore = $state(false);
  let addProject = $state<number | null>(null);
  let addNotes = $state('');
  let busy = $state(false);
  let actionError = $state<string | null>(null);

  const sections: StatusSections = $derived(groupTasksByStatus(tasks, Math.floor(Date.now() / 1000)));
  const pickable = $derived(($projects ?? []).filter((p) => !p.project?.system));

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
    orgNames = new Map(orgs.map((o) => [o.id, o.name]));
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
    if ((get(projects) ?? []).length === 0) void loadProjects();
  });
  onDestroy(() => {
    offFilters();
    offChanged();
  });

  async function add() {
    const title = addTitle.trim();
    if (!title || busy) return;
    busy = true;
    const r = await createWorkTask({ title, projectId: addProject, notes: addNotes });
    busy = false;
    if (!r.ok) {
      actionError = readErrorText(r.error);
      return;
    }
    actionError = null;
    addTitle = '';
    addNotes = '';
    addMore = false;
    void load();
  }

  async function start(t: WorkTask) {
    if (t.item_id == null || busy) return;
    busy = true;
    const r = await startWork({ item_id: t.item_id, ...(t.project_id != null ? { project_id: t.project_id } : {}) });
    busy = false;
    if (!r.ok) {
      actionError = readErrorText(r.error);
      return;
    }
    actionError = null;
    selectSessionExplicitly(r.value);
  }

  const canStart = (t: WorkTask) => (t.counts?.active ?? 0) === 0 && t.status_category !== 'done' && t.item_id != null;
  // The same badge as the Grouped tree's row.
  function badge(t: WorkTask): string {
    if (t.kind === 'local') return 'local';
    if (t.kind === 'ref') return 'key';
    return providerInfo(t.provider)?.icon ?? t.provider ?? 'tracker';
  }
  const projectName = (p: (typeof pickable)[number]) =>
    p.project.owner && p.project.owner !== 'local' ? `${p.project.owner}/${p.project.repo}` : p.project.repo;
</script>

<div class="task-list" data-testid="task-list">
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
      title="Project and notes"
      aria-label="Project and notes"
      data-testid="task-add-more"
      aria-expanded={addMore}
      onclick={() => (addMore = !addMore)}>▾</button
    >
  </div>
  {#if addMore}
    <div class="add-more">
      <select aria-label="Project" data-testid="task-add-project" bind:value={addProject}>
        <option value={null}>No project</option>
        {#each pickable as p (p.project.id)}
          <option value={p.project.id}>{projectName(p)}</option>
        {/each}
      </select>
      <textarea
        rows="3"
        placeholder="Notes: the first prompt when you press Start"
        aria-label="Notes"
        data-testid="task-add-notes"
        bind:value={addNotes}
      ></textarea>
    </div>
  {/if}
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
    {@render section('todo', 'To do', sections.todo, true)}
    {@render section('doing', 'Doing', sections.doing, true)}
    {@render section('done', 'Done · last 7 days', sections.done, doneOpen)}
  {/if}
</div>

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
            <div class="row" data-testid="task-row">
              <span class="tb" title={t.tracker_name ?? t.kind}>{badge(t)}</span>
              <button
                class="main"
                type="button"
                aria-current={$selectedTaskId === t.task_id ? 'true' : undefined}
                onclick={() => openTask(t.task_id)}
              >
                <span class="title">
                  {#if t.needs_you}<span class="needs" title="A session needs you" aria-label="needs you">●</span>{/if}
                  {#if t.key}<span class="key">{t.key}</span>{/if}
                  <span class="txt" class:derived={t.title_derived}>{displayTitle(t)}</span>
                </span>
                <span class="meta">
                  {#if t.status_name}<span>{t.status_name}</span>{/if}
                  {#if t.project_label}<span>{t.project_label}</span>{/if}
                  {#if t.org_id != null && orgNames.get(t.org_id)}<span>{orgNames.get(t.org_id)}</span>{/if}
                  <span title="active / past sessions">{t.counts?.active ?? 0} active · {t.counts?.ended ?? 0} past</span>
                </span>
              </button>
              <span class="right">
                {#if (t.open_proposals ?? 0) > 0}
                  <span class="chip prop" data-testid="task-proposals-badge" title="Agent proposals — decide them on the task page"
                    >{t.open_proposals} to review</span
                  >
                {/if}
                {#if canStart(t)}
                  <button
                    class="btn"
                    type="button"
                    data-testid="task-start"
                    disabled={busy || startBlocked !== null}
                    title={startBlocked ?? 'Start a session for this task'}
                    onclick={() => void start(t)}>Start</button
                  >
                {/if}
              </span>
            </div>
            {#if n.children.length > 0}
              <ul class="children">
                {#each n.children as c (c.task_id)}
                  <li class="child" data-testid="task-child">
                    <span class="dot dot--{c.status_category ?? 'todo'}" aria-hidden="true"></span>
                    <button class="txt" type="button" onclick={() => openTask(c.task_id)}>{displayTitle(c)}</button>
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
    font-size: 0.85rem;
  }
  .add {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 0;
  }
  .add input {
    flex: 1;
    min-width: 0;
    height: 26px;
    padding: 0 8px;
    border: 1px dashed var(--control-border);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font: inherit;
  }
  .add-more {
    display: grid;
    gap: 6px;
    padding-bottom: 6px;
  }
  h3 {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 10px 4px 4px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    font-weight: 600;
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
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li.selected > .row {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    padding: 4px;
    border-radius: 4px;
  }
  .row:hover {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .tb {
    font-size: 0.62rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.2rem;
    text-align: center;
    color: var(--fg-muted);
    margin-top: 2px;
  }
  .main {
    min-width: 0;
    display: grid;
    gap: 1px;
    background: none;
    border: 0;
    padding: 0;
    text-align: left;
    color: var(--fg);
    font: inherit;
    cursor: pointer;
  }
  .main:focus-visible,
  .child .txt:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: calc(-1 * var(--ring-w));
  }
  .title {
    display: flex;
    gap: 5px;
    align-items: baseline;
    min-width: 0;
  }
  .txt {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .derived {
    font-style: italic;
    color: var(--fg-muted);
  }
  .key {
    font-family: var(--mono);
    flex: none;
  }
  .needs {
    color: var(--usage-crit, #c62828);
    font-size: 0.55rem;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0 8px;
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  /* Each fact wraps as a whole in a narrow sidebar, never mid-phrase. */
  .meta > span {
    white-space: nowrap;
  }
  .right {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .chip {
    font-size: 0.7rem;
    border-radius: 999px;
    padding: 0 6px;
    white-space: nowrap;
  }
  .prop {
    background: color-mix(in srgb, var(--usage-warn, #b45309) 14%, transparent);
    color: var(--usage-warn, #b45309);
  }
  .agent {
    background: color-mix(in srgb, #7c3aed 12%, transparent);
    color: #7c3aed;
  }
  .children {
    margin: 0 0 4px 24px;
    border-left: 1px solid var(--border);
    padding-left: 8px;
  }
  .child {
    display: grid;
    grid-template-columns: 10px minmax(0, 1fr) auto;
    gap: 6px;
    align-items: center;
    font-size: 0.8rem;
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
    background: var(--usage-ok, #2e7d32);
    border-color: var(--usage-ok, #2e7d32);
  }
  .muted {
    color: var(--fg-muted);
    padding: 6px 4px;
    margin: 0;
  }
  .err {
    color: var(--usage-crit, #c62828);
    padding: 4px;
    margin: 0;
  }
</style>
