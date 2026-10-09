<script lang="ts">
  import Loader from './Loader.svelte';
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import {
    LOCAL_WORK_TITLE_MAX,
    editWorkItem,
    parseAssignees,
    setWorkStatus,
    workTitleError,
    type WorkItemEdit,
    type WorkItemStatus,
  } from './work';
  import { readErrorText, workTask, type TaskDetail } from './work_view';

  // Edit a task written in Fleet: its title, notes, status, assignees and due date.
  // Reads the task itself (the board and the lists hold no notes), and
  // writes only what changed: `edit_work_item` for the text, then
  // `set_work_status` for the status, both Routed, so a paired desktop
  // edits on its hub. A tracker's ticket is not offered here: its text and
  // status are its tracker's.

  let {
    taskId,
    onclose,
    ondone,
  }: {
    /** `item:<id>`. */
    taskId: string;
    onclose: () => void;
    /** After a successful write, before the dialog closes. */
    ondone?: () => void;
  } = $props();

  const STATUSES: { value: WorkItemStatus; label: string }[] = [
    { value: 'todo', label: 'To do' },
    { value: 'in_progress', label: 'Doing' },
    { value: 'done', label: 'Done' },
  ];

  let detail = $state<TaskDetail | null>(null);
  let loadError = $state<string | null>(null);
  let title = $state('');
  let notes = $state('');
  let status = $state<WorkItemStatus>('todo');
  let assignees = $state('');
  let due = $state('');
  let busy = $state(false);
  let failure = $state<string | null>(null);

  function statusOf(raw: string | null | undefined): WorkItemStatus {
    return raw === 'done' || raw === 'in_progress' ? raw : 'todo';
  }

  // What the form started from: only a changed field is written.
  let initial = { title: '', notes: '', status: 'todo' as WorkItemStatus, assignees: [] as string[], due: '' };

  async function load() {
    const r = await workTask(untrack(() => taskId));
    if (!r.ok) {
      loadError = readErrorText(r.error);
      return;
    }
    const d = r.value;
    if (!d?.task) {
      loadError = 'This task no longer exists, or is not visible from here.';
      return;
    }
    detail = d;
    initial = {
      title: d.task.title ?? '',
      notes: d.notes ?? '',
      status: statusOf(d.task.status_category),
      assignees: d.task.assignees ?? [],
      due: d.task.due_at ?? '',
    };
    title = initial.title;
    notes = initial.notes;
    status = initial.status;
    assignees = initial.assignees.join(', ');
    due = initial.due;
  }
  void load();

  const task = $derived(detail?.task ?? null);
  const editable = $derived(task !== null && task.kind === 'local' && task.item_id != null);
  /** A job mirror's notes are its dispatch prompt: shown, never edited. */
  const notesLocked = $derived(task?.origin === 'agent');
  const titleError = $derived(title.trim() === '' ? 'A title is required.' : workTitleError(title));
  const blocked = $derived(
    hubActionBlocked('edit_work_item', $hubStatus, $hubConnection) ??
      hubActionBlocked('set_work_status', $hubStatus, $hubConnection),
  );

  const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
  const changes = $derived.by(() => {
    const edit: WorkItemEdit = {};
    if (title.trim() !== initial.title) edit.title = title;
    if (!notesLocked && notes.trim() !== initial.notes.trim()) edit.notes = notes;
    const people = parseAssignees(assignees);
    if (!sameList(people, initial.assignees)) edit.assignees = people;
    if (due !== initial.due) edit.due_at = due;
    return { edit, status: status !== initial.status ? status : null };
  });
  const dirty = $derived(Object.keys(changes.edit).length > 0 || changes.status !== null);
  const canSubmit = $derived(editable && !busy && blocked === null && titleError === null && dirty);

  async function submit(e?: Event) {
    e?.preventDefault();
    if (!canSubmit || task?.item_id == null) return;
    const itemId = task.item_id;
    const { edit, status: nextStatus } = changes;
    busy = true;
    failure = null;
    if (Object.keys(edit).length > 0) {
      const r = await editWorkItem(itemId, edit);
      if (!r.ok) {
        busy = false;
        failure = readErrorText(r.error);
        return;
      }
      initial = {
        ...initial,
        title: r.value.title,
        notes: r.value.notes ?? '',
        assignees: r.value.assignees ?? [],
        due: r.value.due_at ?? '',
      };
    }
    if (nextStatus !== null) {
      const r = await setWorkStatus(itemId, nextStatus);
      if (!r.ok) {
        busy = false;
        failure = readErrorText(r.error);
        return;
      }
    }
    busy = false;
    ondone?.();
    onclose();
  }
</script>

<Modal title="Edit task" {onclose} width="480px" testid="edit-task-dialog">
  {#if loadError}
    <p class="err" role="alert" data-testid="edit-task-load-error">{loadError}</p>
    <div class="actions"><button type="button" onclick={onclose}>Close</button></div>
  {:else if !task}
    <p class="note" data-testid="edit-task-loading">Loading…</p>
  {:else if !editable}
    <p class="note" data-testid="edit-task-tracker">
      {task.key ?? 'This task'} belongs to {task.tracker_name || task.provider || 'its tracker'}: edit it there.
    </p>
    <div class="actions"><button type="button" onclick={onclose}>Close</button></div>
  {:else}
    <form class="form" onsubmit={submit}>
      {#if task.key}<p class="note">{task.key}</p>{/if}
      <label class="field">
        <span>Title</span>
        <input
          type="text"
          bind:value={title}
          maxlength={LOCAL_WORK_TITLE_MAX * 2}
          data-autofocus=""
          autocomplete="off"
          data-testid="edit-task-title"
          aria-invalid={titleError !== null}
        />
      </label>
      {#if titleError}<p class="err" data-testid="edit-task-title-error">{titleError}</p>{/if}
      <label class="field">
        <span>Description</span>
        <textarea
          rows="6"
          bind:value={notes}
          disabled={notesLocked}
          placeholder="What needs doing, links, context…"
          data-testid="edit-task-notes"
        ></textarea>
      </label>
      {#if notesLocked}
        <p class="note" data-testid="edit-task-notes-locked">A delegated job's description is its prompt and stays as sent.</p>
      {/if}
      <div class="row">
        <label class="field">
          <span>Status</span>
          <select bind:value={status} data-testid="edit-task-status">
            {#each STATUSES as s (s.value)}<option value={s.value}>{s.label}</option>{/each}
          </select>
        </label>
        <label class="field grow">
          <span>Assignees</span>
          <input
            type="text"
            bind:value={assignees}
            placeholder="Names, comma-separated"
            autocomplete="off"
            data-testid="edit-task-assignees"
          />
        </label>
        <label class="field">
          <span>Due</span>
          <input type="date" bind:value={due} data-testid="edit-task-due" />
        </label>
      </div>
      {#if blocked}<p class="err" data-testid="edit-task-blocked">{blocked}</p>{/if}
      {#if failure}<p class="err" role="alert" data-testid="edit-task-error">{failure}</p>{/if}
      <div class="actions">
        <button type="button" onclick={onclose}>Cancel</button>
        <button type="submit" class="primary" disabled={!canSubmit} title={blocked ?? ''} data-testid="edit-task-submit"
          >{#if busy}<Loader name="comet" size={12} class="btn-loader" />{/if}{busy ? 'Saving…' : 'Save'}</button
        >
      </div>
    </form>
  {/if}
</Modal>

<style>
  .form { display: flex; flex-direction: column; gap: 0.6rem; }
  .row { display: flex; gap: 0.6rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; }
  .field.grow { flex: 1; }
  .field span { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field input,
  .field textarea,
  .field select {
    font: inherit;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .field textarea { resize: vertical; min-height: 5rem; }
  .field textarea:disabled { opacity: 0.6; }
  .field input[aria-invalid='true'] { border-color: var(--danger); }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
