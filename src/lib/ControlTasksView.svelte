<!--
  Control's Tasks view (board MCTasks): the tasks Work shows, under Work's
  filters, in Needs you, In progress, Up next and Done this week. One
  `work_tree` read, refreshed when work changes. Read-only: a row opens the
  task in Work, where it is planned and changed. Tracker titles render as
  text.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { onWorkChangedDebounced } from './work';
  import { showTaskInWorkView, workTree, workViewFilters, type WorkTask } from './work_view';
  import { leave } from './destination';
  import { timeAgo } from './session_status';
  import { controlTaskGroups, taskLine, taskTitle, type TaskGroupId } from './control_tasks';
  import ControlFold from './ControlFold.svelte';
  import LoadError from './states/LoadError.svelte';
  import Skeleton from './states/Skeleton.svelte';
  import type { IpcError } from './result';

  let { now = () => Date.now() }: { now?: () => number } = $props();

  let tasks = $state<WorkTask[] | null>(null);
  let error = $state<IpcError | null>(null);
  let loading = $state(false);
  /** Up next and Done start folded, as the board draws them. */
  let open = $state<Record<TaskGroupId, boolean>>({ 'needs-you': true, doing: true, next: false, done: false });

  let seq = 0;
  async function load() {
    const mine = ++seq;
    loading = true;
    const r = await workTree({ filters: { ...get(workViewFilters), archived: true }, limit: 200, per_task: 1 });
    if (mine !== seq) return;
    loading = false;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
  }

  const groups = $derived(tasks ? controlTaskGroups(tasks, Math.floor(now() / 1000)) : []);

  let stop: (() => void) | undefined;
  onMount(() => {
    void load();
    stop = onWorkChangedDebounced(() => void load(), () => 1500, () => 8000);
  });
  onDestroy(() => stop?.());

  function openTask(t: WorkTask) {
    showTaskInWorkView(t.task_id);
    leave('control');
  }
</script>

<div class="tasks" data-testid="control-tasks">
  {#if error && !tasks}
    <LoadError title="Couldn't load tasks" {error} onretry={load} retrying={loading} testid="control-tasks-error" />
  {:else if !tasks}
    <Skeleton rows={4} label="Loading tasks" />
  {:else if tasks.length === 0}
    <p class="empty" data-testid="control-tasks-empty">No tasks under Work's filters. Ask Control to plan one.</p>
  {:else}
    {#each groups as g (g.id)}
      <ControlFold label={g.label} count={g.tasks.length} bind:open={open[g.id]} tone={g.id === 'needs-you' ? 'waiting' : null} testid="control-tasks-{g.id}">
        <ul>
          {#each g.tasks as t (t.task_id)}
            <li>
              <button type="button" class="row" data-testid="control-task-row" title="Open in Work" onclick={() => openTask(t)}>
                <span class="title">{taskTitle(t)}</span>
                <span class="line">{taskLine(t)}</span>
                {#if t.last_activity_at}<span class="age">{timeAgo(t.last_activity_at, now())}</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      </ControlFold>
    {/each}
  {/if}
</div>

<style>
  .tasks {
    display: flex;
    flex-direction: column;
    padding-bottom: var(--space-2);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 0 var(--space-2);
    width: 100%;
    text-align: left;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--fg);
    font: inherit;
    padding: 6px var(--space-2);
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: -2px;
  }
  .title {
    font-size: var(--text-sm);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .line,
  .age {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .line {
    grid-column: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .age {
    grid-column: 2;
    grid-row: 1;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    margin: 0;
    padding: var(--space-3);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
</style>
