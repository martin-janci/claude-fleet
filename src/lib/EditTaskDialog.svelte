<script lang="ts">
  import { untrack } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import { savedWithUndo } from './forms/form_frame';
  import type { IpcError } from './result';
  import { pushError } from './toasts';
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
  import { openExternal } from './open_external';
  import { providerInfo } from './trackers';

  // Edit a task written in Fleet: its title, notes, status, assignees and due date.
  // Reads the task itself (the board and the lists hold no notes), and
  // writes only what changed: `edit_work_item` for the text, then
  // `set_work_status` for the status, both Routed, so a paired desktop
  // edits on its hub. A tracker's ticket is not edited here: its text and
  // status are its tracker's, and fleet writes back only a PR link
  // (decision D3), so the dialog says so and offers "Open in Jira Cloud ↗".
  //
  // The form kit (G1.2) through DialogSheet: the title is checked once the
  // person leaves it, the off Save says why under it, a failure (a hub
  // refusal too) is a banner over the fields with the edit kept, closing a
  // changed form asks "Discard changes?", and a save offers Undo, which
  // writes the previous values back.

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
  let failure = $state<string | IpcError | null>(null);
  let titleLeft = $state(false);
  let tried = $state(false);

  function statusOf(raw: string | null | undefined): WorkItemStatus {
    return raw === 'done' || raw === 'in_progress' ? raw : 'todo';
  }

  // What the form started from: only a changed field is written.
  let initial = { title: '', notes: '', status: 'todo' as WorkItemStatus, assignees: [] as string[], due: '', epic: false };
  let epic = $state(false);

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
      epic: d.task.epic === true,
    };
    epic = initial.epic;
    title = initial.title;
    notes = initial.notes;
    status = initial.status;
    assignees = initial.assignees.join(', ');
    due = initial.due;
  }
  void load();

  const task = $derived(detail?.task ?? null);
  /** The tracker's name for a ticket: "Jira", else the connection's name. */
  const trackerLabel = $derived(
    providerInfo(task?.provider)?.label ?? (task?.tracker_name || task?.provider || 'its tracker'),
  );
  const editable = $derived(task !== null && task.kind === 'local' && task.item_id != null);
  /** A job mirror's notes are its dispatch prompt: shown, never edited. */
  const notesLocked = $derived(task?.origin === 'agent');
  /** An epic sits at the top (sprints design §3): a filed task or a job
   *  cannot become one. */
  const epicOffered = $derived(task !== null && !task.parent_task_id && task.origin !== 'agent');
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
    if (epicOffered && epic !== initial.epic) edit.epic = epic;
    return { edit, status: status !== initial.status ? status : null };
  });
  const dirty = $derived(Object.keys(changes.edit).length > 0 || changes.status !== null);
  const canSubmit = $derived(editable && !busy && blocked === null && titleError === null && dirty);
  const shownTitleError = $derived(titleLeft || tried ? titleError : null);
  /** Why Save is off, under it. */
  const why = $derived(blocked ?? titleError ?? (dirty ? null : 'Nothing changed yet.'));

  /** A refusal stays an `IpcError` so the banner can say the hub refused. */
  const failed = (e: IpcError): string | IpcError => (e.code === 'E_FORBIDDEN' ? e : readErrorText(e));

  /** Put the fields this save changed back to what they were. */
  async function undo(itemId: number, before: typeof initial, edit: typeof changes.edit, nextStatus: WorkItemStatus | null) {
    const back: WorkItemEdit = {};
    if (edit.title !== undefined) back.title = before.title;
    if (edit.notes !== undefined) back.notes = before.notes;
    if (edit.assignees !== undefined) back.assignees = before.assignees;
    if (edit.due_at !== undefined) back.due_at = before.due;
    if (edit.epic !== undefined) back.epic = before.epic;
    if (Object.keys(back).length > 0) {
      const r = await editWorkItem(itemId, back);
      if (!r.ok) return void pushError(r.error, 'Undo failed');
    }
    if (nextStatus !== null) {
      const r = await setWorkStatus(itemId, before.status);
      if (!r.ok) return void pushError(r.error, 'Undo failed');
    }
    ondone?.();
  }

  async function submit() {
    if (!canSubmit || task?.item_id == null) return;
    const itemId = task.item_id;
    const { edit, status: nextStatus } = changes;
    const before = { ...initial };
    busy = true;
    failure = null;
    if (Object.keys(edit).length > 0) {
      const r = await editWorkItem(itemId, edit);
      if (!r.ok) {
        busy = false;
        failure = failed(r.error);
        return;
      }
      initial = {
        ...initial,
        title: r.value.title,
        notes: r.value.notes ?? '',
        assignees: r.value.assignees ?? [],
        due: r.value.due_at ?? '',
        epic: edit.epic ?? initial.epic,
      };
    }
    if (nextStatus !== null) {
      const r = await setWorkStatus(itemId, nextStatus);
      if (!r.ok) {
        busy = false;
        failure = failed(r.error);
        return;
      }
    }
    busy = false;
    ondone?.();
    onclose();
    savedWithUndo('Task saved.', () => undo(itemId, before, edit, nextStatus));
  }
</script>

<DialogSheet
  title="Edit task"
  lead={task?.key ? `${task.key}: changes save to this task in Fleet.` : 'Changes save to this task in Fleet.'}
  verb="Save"
  busyVerb="Saving…"
  onconfirm={() => void submit()}
  {onclose}
  canConfirm={canSubmit}
  {busy}
  error={loadError ?? failure}
  {dirty}
  oninvalid={() => (tried = true)}
  testid="edit-task-dialog"
  confirmTestid="edit-task-submit"
  errorTestid={loadError ? 'edit-task-load-error' : 'edit-task-error'}
  confirmTitle={editable ? why : null}
>
  {#snippet secondary()}
    {#if task && !editable && task.url}
      <!-- The quiet footer link (FormsWork): the ticket's own page. -->
      <button
        type="button"
        class="btn btn--quiet open-ext"
        data-testid="edit-task-open-tracker"
        onclick={() => void openExternal(task?.url ?? '')}>Open in {trackerLabel} ↗</button
      >
    {/if}
  {/snippet}
  {#if loadError}
    <!-- Nothing to edit: the banner says why. -->
  {:else if !task}
    <p class="note" data-testid="edit-task-loading">Loading…</p>
  {:else if !editable}
    <p class="note" data-testid="edit-task-tracker">
      {task.key ?? 'This task'} belongs to {trackerLabel}: edit it there. Fleet writes nothing back to a
      tracker but a pull request link.
    </p>
  {:else}
    <label class="field">
      <span class="field-label">Title</span>
      <input
        type="text"
        bind:value={title}
        maxlength={LOCAL_WORK_TITLE_MAX * 2}
        data-autofocus=""
        autocomplete="off"
        data-testid="edit-task-title"
        aria-invalid={shownTitleError !== null}
        onblur={() => (titleLeft = true)}
      />
      {#if shownTitleError}<span class="err" data-testid="edit-task-title-error">{shownTitleError}</span>{/if}
    </label>
    <label class="field">
      <span class="field-label">Description</span>
      <textarea
        rows="6"
        bind:value={notes}
        disabled={notesLocked}
        placeholder="What needs doing, links, context…"
        data-testid="edit-task-notes"
      ></textarea>
      {#if notesLocked}
        <span class="field-note" data-testid="edit-task-notes-locked"
          >A delegated job's description is its prompt and stays as sent.</span
        >
      {/if}
    </label>
    <div class="row">
      <label class="field">
        <span class="field-label">Status</span>
        <select bind:value={status} data-testid="edit-task-status">
          {#each STATUSES as s (s.value)}<option value={s.value}>{s.label}</option>{/each}
        </select>
      </label>
      <label class="field grow">
        <span class="field-label">Assignees</span>
        <input
          type="text"
          bind:value={assignees}
          placeholder="Names, comma-separated"
          autocomplete="off"
          data-testid="edit-task-assignees"
        />
      </label>
      <label class="field">
        <span class="field-label">Due</span>
        <input type="date" bind:value={due} data-testid="edit-task-due" />
      </label>
    </div>
    {#if epicOffered}
      <label class="check">
        <input type="checkbox" bind:checked={epic} data-testid="edit-task-epic" />
        <span>Epic: other tasks are filed under it, and it shows how many of them are done</span>
      </label>
    {/if}
  {/if}
</DialogSheet>

<style>
  .row { display: flex; gap: var(--space-3); }
  .check { display: flex; gap: var(--space-2); align-items: baseline; font-size: var(--text-xs); }
  .field.grow { flex: 1; }
  .field textarea:disabled { opacity: 0.6; }
  .field input[aria-invalid='true'] { border-color: var(--danger); }
  .note { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0; }
  .err { color: var(--danger); font-size: var(--text-xs); margin: 0; }
</style>
