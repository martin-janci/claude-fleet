<script lang="ts">
  // New task (gap plan G2.1, the FormsWork board): the list's inline
  // "+ New task" as a dialog, opened by ⌘N in Work and by the list's ▾.
  // Title, project and notes, then "Start a session for it now", which
  // opens the new task's start menu next (`new_task.ts`).
  //
  // The task stays in Fleet: fleet writes nothing to a tracker but a PR
  // link (decision D3), so there is no Tracker field to pick one with.
  //
  // The form kit (G1.2) through DialogSheet: the title is checked once the
  // person leaves it, the off verb says why under it, a failure is a banner
  // over the fields with the input kept, ⌘↵ creates, and closing a changed
  // form asks "Discard changes?".
  import { onMount, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import DialogSheet from './DialogSheet.svelte';
  import { savedWithUndo } from './forms/form_frame';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { requestStartAsk } from './new_task';
  import { loadProjects, projects } from './projects';
  import type { IpcError } from './result';
  import { LOCAL_WORK_TITLE_MAX, createWorkTask, workTitleError } from './work';
  import { openTask, readErrorText } from './work_view';

  let {
    initialTitle = '',
    onclose,
    ondone,
  }: {
    /** What the list's inline field held when ▾ opened the dialog. */
    initialTitle?: string;
    onclose: () => void;
    /** After the task is made, before the dialog closes. */
    ondone?: () => void;
  } = $props();

  let title = $state(untrack(() => initialTitle));
  let projectId = $state<number | null>(null);
  let notes = $state('');
  let startNow = $state(false);
  let busy = $state(false);
  let failure = $state<string | IpcError | null>(null);
  let titleLeft = $state(false);
  let tried = $state(false);

  onMount(() => {
    if ((get(projects) ?? []).length === 0) void loadProjects();
  });

  const pickable = $derived(($projects ?? []).filter((p) => !p.project?.system));
  const projectName = (p: (typeof pickable)[number]) =>
    p.project.owner && p.project.owner !== 'local' ? `${p.project.owner}/${p.project.repo}` : p.project.repo;

  const titleError = $derived(title.trim() === '' ? 'A title is required.' : workTitleError(title));
  /** Creating is Routed (`work_link`), never gated here; a start is. */
  const blocked = $derived(startNow ? hubActionBlocked('start_work', $hubStatus, $hubConnection) : null);
  const dirty = $derived(title.trim() !== initialTitle.trim() || notes.trim() !== '' || projectId !== null || startNow);
  const canSubmit = $derived(!busy && blocked === null && titleError === null);
  const shownTitleError = $derived(titleLeft || tried ? titleError : null);
  const failed = (e: IpcError): string | IpcError => (e.code === 'E_FORBIDDEN' ? e : readErrorText(e));

  async function submit() {
    if (!canSubmit) return;
    busy = true;
    failure = null;
    const r = await createWorkTask({ title, projectId, notes });
    busy = false;
    if (!r.ok) {
      failure = failed(r.error);
      return;
    }
    const taskId = `item:${r.value.id}`;
    ondone?.();
    onclose();
    if (startNow) {
      // The task page's Work button (or its list row's) opens the menu.
      requestStartAsk(taskId);
      openTask(taskId);
    } else {
      savedWithUndo(`Task created${r.value.key ? `: ${r.value.key}` : ''}.`);
    }
  }
</script>

<DialogSheet
  title="New task"
  lead="Adds a task to Work. It stays in Fleet: it is not sent to a tracker."
  verb="Create task"
  busyVerb="Creating…"
  onconfirm={() => void submit()}
  {onclose}
  canConfirm={canSubmit}
  {busy}
  error={failure}
  {dirty}
  oninvalid={() => (tried = true)}
  testid="new-task-dialog"
  confirmTestid="new-task-submit"
  errorTestid="new-task-error"
  confirmTitle={blocked ?? titleError}
>
  <label class="field">
    <span class="field-label">Title</span>
    <input
      type="text"
      bind:value={title}
      maxlength={LOCAL_WORK_TITLE_MAX * 2}
      data-autofocus=""
      autocomplete="off"
      placeholder="What needs doing?"
      data-testid="new-task-title"
      aria-invalid={shownTitleError !== null}
      onblur={() => (titleLeft = true)}
    />
    {#if shownTitleError}<span class="err" data-testid="new-task-title-error">{shownTitleError}</span>{/if}
  </label>
  <label class="field">
    <span class="field-label">Project</span>
    <select bind:value={projectId} data-testid="new-task-project">
      <option value={null}>No project</option>
      {#each pickable as p (p.project.id)}
        <option value={p.project.id}>{projectName(p)}</option>
      {/each}
    </select>
  </label>
  <label class="field">
    <span class="field-label">Notes <span class="opt">optional</span></span>
    <textarea
      rows="3"
      bind:value={notes}
      placeholder="The first prompt when you press Start"
      data-testid="new-task-notes"
    ></textarea>
  </label>
  <label class="check">
    <input type="checkbox" bind:checked={startNow} data-testid="new-task-start-now" />
    <span>Start a session for it now <span class="opt">· opens the start menu next</span></span>
  </label>
</DialogSheet>

<style>
  .field input[aria-invalid='true'] { border-color: var(--danger); }
  .opt { color: var(--fg-muted); font-weight: 400; }
  .check { display: flex; gap: var(--space-2); align-items: center; font-size: var(--text-xs); }
  .err { color: var(--danger); font-size: var(--text-xs); margin: 0; }
</style>
