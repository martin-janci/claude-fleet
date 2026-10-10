<!--
  Control's Tasks view (gap plan G3.10, board MCTasks): the Work tree's
  tasks beside the chat, by status (Needs you, In progress, Up next, Done
  this week), filtered by Mine and Claude. Tick tasks for Start new (N),
  which starts a session on each one without a live session, Assign…,
  and Done; a tracker's ticket changes in its tracker, so Assign and Done
  skip it and say so. A row opens the task in Work.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { workChanged, editWorkItem, parseAssignees, setWorkStatus, ownerDueChip } from './work';
  import { startWork } from './trackers';
  import { readErrorText, showTaskInWorkView, taskLabel, taskStatus, workTree, type WorkTask } from './work_view';
  import { leave } from './destination';
  import {
    TASK_SECTIONS,
    TASK_SECTION_LABELS,
    isNativeTask,
    startable,
    tasksBySection,
    type TaskChips,
  } from './control_views';
  import type { IpcError } from './result';

  let chips = $state<TaskChips>({ mine: false, claude: false });
  let tasks = $state<WorkTask[]>([]);
  let loaded = $state(false);
  let loadError = $state<IpcError | null>(null);
  let picked = $state<string[]>([]);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let assigning = $state(false);
  let assignee = $state('');

  async function load() {
    const r = await workTree({ filters: { mine: chips.mine || undefined, archived: true }, limit: 200, per_task: 1 });
    loaded = true;
    if (!r.ok) {
      loadError = r.error;
      return;
    }
    loadError = null;
    tasks = r.value.tasks;
    picked = picked.filter((id) => tasks.some((t) => t.task_id === id));
  }

  onMount(() => {
    void load();
    let first = true;
    return workChanged.subscribe(() => {
      if (first) first = false;
      else void load();
    });
  });

  const nowSec = Math.floor(Date.now() / 1000);
  const sections = $derived(tasksBySection(tasks, chips, nowSec));
  const shownCount = $derived(TASK_SECTIONS.reduce((n, s) => n + sections[s].length, 0));
  const selected = $derived(tasks.filter((t) => picked.includes(t.task_id)));
  const toStart = $derived(startable(selected));
  const native = $derived(selected.filter(isNativeTask));

  function chip(k: keyof TaskChips) {
    chips = { ...chips, [k]: !chips[k] };
    if (k === 'mine') void load();
  }

  function toggle(id: string) {
    picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
  }

  function open(t: WorkTask) {
    showTaskInWorkView(t.task_id);
    leave('control');
  }

  function skipped(what: string): string {
    const n = selected.length - native.length;
    return n > 0 ? ` ${n} tracker ${n === 1 ? 'ticket' : 'tickets'} ${what} in ${n === 1 ? 'its' : 'their'} tracker.` : '';
  }

  async function startNew() {
    busy = true;
    note = null;
    let ok = 0;
    const failed: string[] = [];
    for (const t of toStart) {
      const r = await startWork(t.item_id != null ? { item_id: t.item_id, with_brief: true } : { reference: t.key!, with_brief: true });
      if (r.ok) ok++;
      else failed.push(`${taskLabel(t)}: ${r.error.message}`);
    }
    busy = false;
    note = [ok ? `Started ${ok} ${ok === 1 ? 'session' : 'sessions'}.` : '', ...failed].filter(Boolean).join(' ');
    picked = [];
  }

  async function markDone() {
    busy = true;
    note = null;
    const fails: string[] = [];
    for (const t of native) {
      const r = await setWorkStatus(t.item_id, 'done');
      if (!r.ok) fails.push(`${taskLabel(t)}: ${r.error.message}`);
    }
    busy = false;
    note = (fails.join(' ') || `Moved ${native.length} to Done.`) + skipped('change');
    picked = [];
  }

  async function assign() {
    const who = parseAssignees(assignee);
    if (who.length === 0) return;
    busy = true;
    note = null;
    const fails: string[] = [];
    for (const t of native) {
      const r = await editWorkItem(t.item_id, { assignees: who });
      if (!r.ok) fails.push(`${taskLabel(t)}: ${r.error.message}`);
    }
    busy = false;
    assigning = false;
    assignee = '';
    note = (fails.join(' ') || `Assigned ${native.length} to ${who.join(', ')}.`) + skipped('are assigned');
    picked = [];
  }
</script>

<div class="tasks" data-testid="control-tasks">
  <div class="bar">
    <div class="chips" role="group" aria-label="Filters">
      <button type="button" class="chip" aria-pressed={chips.mine} data-testid="control-tasks-mine" onclick={() => chip('mine')}>Mine</button>
      <button type="button" class="chip" aria-pressed={chips.claude} data-testid="control-tasks-claude" title="An agent session is on it now" onclick={() => chip('claude')}
        >Claude</button
      >
    </div>
    <span class="muted">Group: status</span>
  </div>

  {#if picked.length > 0}
    <div class="bulk" role="group" aria-label="{picked.length} selected" data-testid="control-tasks-bulk">
      <button type="button" class="btn" data-testid="control-tasks-start" disabled={busy || toStart.length === 0} onclick={startNew}
        >Start new ({toStart.length})</button
      >
      <button type="button" class="btn" data-testid="control-tasks-assign" disabled={busy || native.length === 0} onclick={() => (assigning = !assigning)}
        >Assign…</button
      >
      <button type="button" class="btn" data-testid="control-tasks-done" disabled={busy || native.length === 0} onclick={markDone}>Done</button>
      <button type="button" class="btn quiet" onclick={() => (picked = [])}>Clear</button>
    </div>
    {#if assigning}
      <form
        class="assign"
        onsubmit={(e) => {
          e.preventDefault();
          void assign();
        }}
      >
        <label
          >Assign to <input bind:value={assignee} placeholder="Name, or names with commas" data-testid="control-tasks-assignee" /></label
        >
        <button type="submit" class="btn" disabled={busy || parseAssignees(assignee).length === 0} data-testid="control-tasks-assign-go">Assign</button>
      </form>
    {/if}
  {/if}
  {#if note}<p class="note" role="status" data-testid="control-tasks-note">{note}</p>{/if}

  {#if loadError}
    <p class="muted" role="alert" data-testid="control-tasks-error">
      {readErrorText(loadError)} <button type="button" class="btn quiet" onclick={() => void load()}>Retry</button>
    </p>
  {:else if loaded && shownCount === 0}
    <p class="muted empty" data-testid="control-tasks-empty">
      {chips.mine || chips.claude ? 'No tasks match these filters.' : 'No tasks yet. Ask Control to plan one, or type /task in the chat.'}
    </p>
  {:else}
    {#each TASK_SECTIONS as sec (sec)}
      {#if sections[sec].length > 0}
        <section aria-label={TASK_SECTION_LABELS[sec]} data-testid="control-tasks-section-{sec}">
          <h3>{TASK_SECTION_LABELS[sec]} <span class="muted">{sections[sec].length}</span></h3>
          <ul>
            {#each sections[sec] as t (t.task_id)}
              {@const who = ownerDueChip(t)}
              <li class="row" data-testid="control-task-row">
                <input
                  type="checkbox"
                  aria-label="Select {taskLabel(t)}"
                  checked={picked.includes(t.task_id)}
                  onchange={() => toggle(t.task_id)}
                />
                <button type="button" class="open" title="Open in Work" onclick={() => open(t)}>
                  <span class="name">{taskLabel(t)}</span>
                  <span class="muted"
                    >{taskStatus(t)}{#if (t.counts?.active ?? 0) > 0}{' '}· {t.counts?.active} running{/if}{#if who}{' '}· {who.text}{/if}</span
                  >
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/each}
  {/if}
</div>

<style>
  .tasks {
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
  }
  .bar,
  .bulk,
  .assign {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .chips {
    display: flex;
    gap: 2px;
  }
  .chip,
  .btn {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 8px;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .chip {
    color: var(--fg-muted);
  }
  .chip[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .quiet {
    border-color: transparent;
    color: var(--fg-muted);
  }
  .chip:focus-visible,
  .btn:focus-visible,
  .open:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .assign input {
    font: inherit;
    font-size: var(--text-xs);
  }
  h3 {
    margin: 0;
    padding: var(--space-2) var(--space-3) var(--space-1);
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--fg-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .open {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1 1 auto;
    text-align: left;
    border: 0;
    background: transparent;
    color: var(--fg);
    font: inherit;
    padding: 0;
    cursor: pointer;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .note,
  .empty {
    margin: 0;
    padding: var(--space-2) var(--space-3);
    font-size: var(--text-xs);
  }
</style>
