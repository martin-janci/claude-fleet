<script lang="ts">
  // New task (gap plan G2.1, the FormsWork board): the list's inline
  // "+ New task" as a dialog, opened by ⌘N in Work and by the list's ▾.
  // Title, project, notes and files, then "Start a session for it now",
  // which opens the new task's start menu next (`new_task.ts`).
  //
  // The title can be drafted from the notes (CF-4, `work.draft_task_titles`,
  // off by default): leaving the notes with no title yet asks a model for
  // one, and "Draft title" asks again. A draft goes into an empty title
  // only, marked Drafted with Undo; beside a typed title it is an offer.
  // A draft that fails says why under the field and blocks nothing.
  //
  // Files (CF-4) are a task's attachments (`task_attachments.ts`). A task
  // has no id until it is made, so the dialog holds them, refusing what the
  // task page would, and attaches each one right after the create. A file
  // that then fails leaves the task made: the dialog closes onto the task,
  // and a toast names the file.
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
  import { draftedBy } from './ai_proposal';
  import DialogSheet from './DialogSheet.svelte';
  import DraftedLabel from './DraftedLabel.svelte';
  import { fleetSettings, settingBool, SETTING_KEYS } from './fleet_settings';
  import { savedWithUndo } from './forms/form_frame';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { requestStartAsk } from './new_task';
  import { loadProjects, projects } from './projects';
  import type { IpcError } from './result';
  import { attachRefusal, attachToWork, formatSize, pastedImages } from './task_attachments';
  import { push } from './toasts';
  import { LOCAL_WORK_TITLE_MAX, createWorkTask, draftTaskTitle, workTitleError, type TitleDraft } from './work';
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
  /** The files to attach once the task exists. */
  let files = $state<File[]>([]);
  let filesError = $state<string | null>(null);
  let dragging = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
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
  // ── the drafted title ──
  const draftBlocked = $derived(hubActionBlocked('draft_task_title', $hubStatus, $hubConnection));
  const draftsTitles = $derived(settingBool($fleetSettings, SETTING_KEYS.workDraftTaskTitles) && draftBlocked === null);
  /** The model's last answer, and the notes it was written from. */
  let draft = $state<TitleDraft | null>(null);
  let draftedFrom = $state<string | null>(null);
  let drafting = $state(false);
  let draftError = $state<string | null>(null);
  /** Only the newest request's answer is used. */
  let draftSeq = 0;
  /** The title is the draft, untouched: it shows the pill and Undo. */
  const showingDraft = $derived(draft !== null && title === draft.title);

  async function draftTitle() {
    const from = notes.trim();
    if (!draftsTitles || from === '') return;
    const seq = ++draftSeq;
    drafting = true;
    draftError = null;
    const r = await draftTaskTitle(from);
    if (seq !== draftSeq) return;
    drafting = false;
    draftedFrom = from;
    if (!r.ok) {
      draft = null;
      draftError = readErrorText(r.error);
      return;
    }
    draft = r.value;
    // Never over what the person typed: there it stays an offer.
    if (title.trim() === '') title = r.value.title;
  }

  /** Leaving the notes: draft once per text, and only for an empty title. */
  function onNotesLeft() {
    if (title.trim() !== '' || drafting || notes.trim() === draftedFrom) return;
    void draftTitle();
  }

  function undoDraft() {
    title = '';
    draft = null;
  }

  const attachBlocked = $derived(hubActionBlocked('attach_to_work', $hubStatus, $hubConnection));
  const dirty = $derived(
    title.trim() !== initialTitle.trim() || notes.trim() !== '' || projectId !== null || startNow || files.length > 0,
  );
  const canSubmit = $derived(!busy && blocked === null && titleError === null);
  const shownTitleError = $derived(titleLeft || tried ? titleError : null);
  const failed = (e: IpcError): string | IpcError => (e.code === 'E_FORBIDDEN' ? e : readErrorText(e));

  /** Hold `picked` for the create; the first refusal is shown. */
  function addFiles(picked: File[]) {
    if (picked.length === 0) return;
    filesError = attachBlocked;
    if (attachBlocked) return;
    const kept: File[] = [];
    for (const f of picked) {
      const refusal = attachRefusal(f);
      if (refusal) filesError ??= refusal;
      else kept.push(f);
    }
    files = [...files, ...kept];
  }

  function removeFile(i: number) {
    files = files.filter((_, at) => at !== i);
    filesError = null;
  }

  function onPaste(e: ClipboardEvent) {
    const images = pastedImages(e);
    if (images.length === 0) return;
    e.preventDefault();
    addFiles(images);
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    addFiles(Array.from(e.dataTransfer?.files ?? []));
  }

  /** Attach the held files to the new task; the names of those that failed. */
  async function attachAll(itemId: number): Promise<string[]> {
    const failed: string[] = [];
    for (const f of files) {
      const r = await attachToWork(itemId, f);
      if (!r.ok) failed.push(`${f.name} (${readErrorText(r.error)})`);
    }
    return failed;
  }

  async function submit() {
    if (!canSubmit) return;
    busy = true;
    failure = null;
    const r = await createWorkTask({ title, projectId, notes });
    if (!r.ok) {
      busy = false;
      failure = failed(r.error);
      return;
    }
    const notAttached = await attachAll(r.value.id);
    busy = false;
    const taskId = `item:${r.value.id}`;
    const made = `Task created${r.value.key ? `: ${r.value.key}` : ''}`;
    ondone?.();
    onclose();
    if (startNow) {
      // The task page's Work button (or its list row's) opens the menu.
      requestStartAsk(taskId);
      openTask(taskId);
    }
    if (notAttached.length > 0) {
      // The task exists; its page is where the file is added again.
      if (!startNow) openTask(taskId);
      push({ kind: 'error', message: `${made}, but not attached: ${notAttached.join(', ')}. Add it on the task.` });
    } else if (!startNow) {
      savedWithUndo(`${made}.`);
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
  {#if draftsTitles}
    <p class="draft" aria-live="polite">
      {#if drafting}
        <span class="opt" data-testid="new-task-drafting">Drafting a title from the notes…</span>
      {:else if showingDraft && draft}
        <span data-testid="new-task-drafted">
          <DraftedLabel testid="new-task-drafted-label" />
          <span class="opt">{draftedBy(draft.model, draft.host_alias, 'from the notes')}</span>
        </span>
        <button type="button" class="link" data-testid="new-task-draft-undo" onclick={undoDraft}>Undo</button>
        <button type="button" class="link" data-testid="new-task-draft-again" onclick={() => { title = ''; void draftTitle(); }}
          >Regenerate</button
        >
      {:else}
        {#if draft && title.trim() !== draft.title}
          <button type="button" class="link" data-testid="new-task-use-draft" onclick={() => (title = draft?.title ?? title)}
            >Use the drafted title “{draft.title}”</button
          >
        {/if}
        {#if draftError}<span class="opt" data-testid="new-task-draft-error">No title drafted: {draftError}</span>{/if}
        <button
          type="button"
          class="link"
          data-testid="new-task-draft-title"
          disabled={notes.trim() === ''}
          title={notes.trim() === '' ? 'Write the notes first: the title is drafted from them.' : 'Draft the title from the notes'}
          onclick={() => void draftTitle()}>Draft title</button
        >
      {/if}
    </p>
  {/if}
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
      onblur={onNotesLeft}
    ></textarea>
  </label>
  <div
    class="field files"
    class:dragging
    role="group"
    aria-label="Attachments"
    data-testid="new-task-files"
    onpaste={onPaste}
    ondragover={(e) => {
      if (!e.dataTransfer?.types?.includes('Files')) return;
      e.preventDefault();
      dragging = true;
    }}
    ondragleave={() => (dragging = false)}
    ondrop={onDrop}
  >
    <span class="field-label files-head">
      <span>Attachments <span class="opt">optional</span></span>
      <button
        class="btn btn--quiet"
        type="button"
        data-testid="new-task-files-add"
        disabled={attachBlocked !== null || busy}
        title={attachBlocked ?? 'Add files (or drop them here, or paste an image)'}
        onclick={() => fileInput?.click()}>+ Add file</button
      >
    </span>
    <input
      bind:this={fileInput}
      class="file-input"
      type="file"
      multiple
      tabindex="-1"
      aria-hidden="true"
      data-testid="new-task-files-input"
      onchange={(e) => {
        const el = e.currentTarget as HTMLInputElement;
        const picked = Array.from(el.files ?? []);
        el.value = '';
        addFiles(picked);
      }}
    />
    {#if filesError}<span class="err" role="alert" data-testid="new-task-files-error">{filesError}</span>{/if}
    {#if files.length === 0}
      <span class="drop">Drop files here or paste an image. They stay in fleet; nothing is sent to a tracker.</span>
    {:else}
      <ul class="file-list">
        {#each files as f, i (i)}
          <li data-testid="new-task-file">
            <span class="file-name" title={f.name}>{f.name}</span>
            <span class="opt">{formatSize(f.size)}</span>
            <button
              class="btn btn--quiet"
              type="button"
              data-testid="new-task-file-remove"
              aria-label="Remove {f.name}"
              disabled={busy}
              onclick={() => removeFile(i)}>Remove</button
            >
          </li>
        {/each}
      </ul>
    {/if}
  </div>
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
  .draft { display: flex; flex-wrap: wrap; gap: var(--space-2); align-items: center; margin: 0; font-size: var(--text-2xs); }
  .link { background: none; border: 0; padding: 0; color: var(--accent); font: inherit; cursor: pointer; }
  .link:disabled { color: var(--fg-muted); cursor: not-allowed; }
  .files { border-radius: var(--radius-md); outline: 1px dashed transparent; outline-offset: 4px; }
  .files.dragging { outline-color: var(--accent); background: var(--accent-soft); }
  .files-head { display: flex; align-items: center; justify-content: space-between; }
  .file-input { display: none; }
  .drop {
    padding: var(--space-2);
    border: 1px dashed var(--border);
    border-radius: var(--radius-md);
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .file-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: var(--space-1); }
  .file-list li { display: flex; gap: var(--space-2); align-items: center; font-size: var(--text-xs); }
  .file-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
