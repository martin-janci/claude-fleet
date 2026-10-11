<!-- A task's Attachments (migration 165): files and images kept in fleet,
     never written to a tracker. Add with the button, by dropping files on
     the section, or by pasting an image anywhere on the task page; images
     show as thumbnails that open in a lightbox, other files download. Each
     card names who added it and when; its author may delete it. -->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { timeAgo } from './session_status';
  import { commentAuthor } from './task_detail';
  import Icon from './kit/Icon.svelte';
  import {
    attachToWork,
    deleteWorkAttachment,
    formatSize,
    isImage,
    pastedImages,
    revokeAttachmentUrls,
    workAttachmentBlobUrl,
  } from './task_attachments';
  import { readErrorText, type TaskAttachment } from './work_view';

  let {
    itemId,
    attachments = [],
    /** Where a paste is listened for (the task page); the section itself
     *  when omitted. */
    pasteTarget = null,
    onchange,
  }: {
    itemId: number;
    attachments?: TaskAttachment[];
    pasteTarget?: HTMLElement | null;
    onchange?: (rows: TaskAttachment[]) => void;
  } = $props();

  const addBlocked = $derived(hubActionBlocked('attach_to_work', $hubStatus, $hubConnection));
  const deleteBlocked = $derived(hubActionBlocked('delete_work_attachment', $hubStatus, $hubConnection));

  // What was added or deleted here, shown at once; the write's bump re-reads
  // the whole detail, which hands a fresh `attachments` in.
  let added = $state<TaskAttachment[]>([]);
  let removed = $state<Set<number>>(new Set());
  const rows = $derived.by(() => {
    const seen = new Set(attachments.map((a) => a.id));
    return [...added.filter((a) => !seen.has(a.id)), ...attachments].filter((a) => !removed.has(a.id));
  });

  let busy = $state(0);
  let error = $state<string | null>(null);
  let confirmDelete = $state<number | null>(null);
  let dragging = $state(false);
  let lightbox = $state<TaskAttachment | null>(null);
  let input = $state<HTMLInputElement | null>(null);
  let section = $state<HTMLElement | null>(null);
  let thumbs = $state<Record<number, string>>({});
  let thumbErrors = $state<Record<number, boolean>>({});

  $effect(() => {
    for (const a of rows) {
      if (!isImage(a.mime) || thumbs[a.id] || thumbErrors[a.id]) continue;
      void workAttachmentBlobUrl(a.id).then((r) => {
        if (r.ok) thumbs = { ...thumbs, [a.id]: r.value };
        else thumbErrors = { ...thumbErrors, [a.id]: true };
      });
    }
  });

  /** Upload `files`, each on its own; the first refusal is shown. */
  export async function upload(files: File[]): Promise<void> {
    if (files.length === 0) return;
    if (addBlocked) {
      error = addBlocked;
      return;
    }
    error = null;
    for (const f of files) {
      busy++;
      const r = await attachToWork(itemId, f);
      busy--;
      if (!r.ok) {
        error = readErrorText(r.error);
        continue;
      }
      added = [{ ...r.value, mine: true }, ...added];
    }
    onchange?.(rows);
  }

  async function remove(id: number) {
    confirmDelete = null;
    error = null;
    const r = await deleteWorkAttachment(id);
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    removed = new Set([...removed, id]);
    if (lightbox?.id === id) lightbox = null;
    onchange?.(rows);
  }

  async function download(a: TaskAttachment) {
    error = null;
    const r = await workAttachmentBlobUrl(a.id);
    if (!r.ok) {
      error = readErrorText(r.error);
      return;
    }
    const link = document.createElement('a');
    link.href = r.value;
    link.download = a.name;
    link.rel = 'noopener';
    document.body.appendChild(link);
    link.click();
    link.remove();
  }

  function onPaste(e: Event) {
    const files = pastedImages(e as ClipboardEvent);
    if (files.length === 0) return;
    e.preventDefault();
    void upload(files);
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    void upload(Array.from(e.dataTransfer?.files ?? []));
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && lightbox) {
      e.preventDefault();
      lightbox = null;
    }
  }

  $effect(() => {
    const target = pasteTarget ?? section;
    if (!target) return;
    target.addEventListener('paste', onPaste);
    return () => target.removeEventListener('paste', onPaste);
  });
  onDestroy(revokeAttachmentUrls);
</script>

<svelte:window onkeydown={onKey} />

<section
  class="attachments"
  class:dragging
  aria-label="Attachments"
  data-testid="task-attachments"
  bind:this={section}
  ondragover={(e) => {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    e.preventDefault();
    dragging = true;
  }}
  ondragleave={() => (dragging = false)}
  ondrop={onDrop}
>
  <h3>
    Attachments <span class="n" data-testid="task-attachments-count">{rows.length}</span>
    <button
      class="btn btn--quiet"
      type="button"
      data-testid="task-attachments-add"
      disabled={addBlocked !== null || busy > 0}
      title={addBlocked ?? 'Add files (or drop them here, or paste an image)'}
      onclick={() => input?.click()}>{busy > 0 ? 'Adding…' : '+ Add file'}</button
    >
  </h3>
  <input
    bind:this={input}
    class="file-input"
    type="file"
    multiple
    tabindex="-1"
    aria-hidden="true"
    data-testid="task-attachments-input"
    onchange={(e) => {
      const el = e.currentTarget as HTMLInputElement;
      const files = Array.from(el.files ?? []);
      el.value = '';
      void upload(files);
    }}
  />
  {#if error}<p class="err" role="alert" data-testid="task-attachments-error">{error}</p>{/if}
  {#if rows.length === 0}
    <p class="muted drop" data-testid="task-attachments-empty">Drop files here or paste an image. They stay in fleet; nothing is sent to a tracker.</p>
  {:else}
    <ul class="grid">
      {#each rows as a (a.id)}
        <li class="card" data-testid="task-attachment">
          {#if isImage(a.mime)}
            <button class="thumb" type="button" title="Open {a.name}" data-testid="task-attachment-open" onclick={() => (lightbox = a)}>
              {#if thumbs[a.id]}
                <img src={thumbs[a.id]} alt={a.name} />
              {:else}
                <span class="muted small">{thumbErrors[a.id] ? 'Not shown' : 'Loading…'}</span>
              {/if}
            </button>
          {:else}
            <button class="thumb file" type="button" title="Download {a.name}" data-testid="task-attachment-download" onclick={() => void download(a)}>
              <Icon name="file" size={24} />
            </button>
          {/if}
          <div class="meta">
            <span class="name" title={a.name}>{a.name}</span>
            <span class="muted small">{formatSize(a.size)} · {commentAuthor(a)} · {timeAgo(a.created_at)}</span>
            {#if a.mine}
              <span class="acts">
                {#if confirmDelete === a.id}
                  <button class="btn btn--crit" type="button" data-testid="task-attachment-delete-confirm" onclick={() => void remove(a.id)}>Delete</button>
                  <button class="btn btn--quiet" type="button" onclick={() => (confirmDelete = null)}>Keep</button>
                {:else}
                  <button
                    class="btn btn--quiet"
                    type="button"
                    data-testid="task-attachment-delete"
                    disabled={deleteBlocked !== null}
                    title={deleteBlocked ?? 'Delete your attachment'}
                    onclick={() => (confirmDelete = a.id)}>Delete…</button
                  >
                {/if}
              </span>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if lightbox}
  <div
    class="lightbox"
    role="dialog"
    aria-modal="true"
    aria-label={lightbox.name}
    data-testid="task-attachment-lightbox"
    tabindex="-1"
    onclick={(e) => {
      if (e.target === e.currentTarget) lightbox = null;
    }}
    onkeydown={onKey}
  >
    <figure>
      {#if thumbs[lightbox.id]}<img src={thumbs[lightbox.id]} alt={lightbox.name} />{/if}
      <figcaption>
        <span>{lightbox.name}</span>
        <button class="btn btn--quiet" type="button" data-testid="task-attachment-lightbox-close" onclick={() => (lightbox = null)}>Close</button>
      </figcaption>
    </figure>
  </div>
{/if}

<style>
  .attachments {
    border-radius: var(--radius-md);
    outline: 1px dashed transparent;
    outline-offset: 4px;
  }
  .attachments.dragging {
    outline-color: var(--accent);
    background: var(--accent-soft);
  }
  h3 {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0 0 6px;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  h3 .btn {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
  }
  .n {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }
  .file-input {
    display: none;
  }
  .drop {
    margin: 0;
    padding: var(--space-2);
    border: 1px dashed var(--border);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
  }
  .err {
    margin: 0 0 6px;
    color: var(--danger);
    font-size: var(--text-xs);
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: var(--space-2);
  }
  .card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    overflow: hidden;
  }
  .thumb {
    display: grid;
    place-items: center;
    height: 96px;
    padding: 0;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
  }
  .thumb img {
    max-width: 100%;
    max-height: 96px;
    object-fit: contain;
  }
  .thumb:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px var(--space-2);
    font-size: var(--text-xs);
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }
  .acts {
    display: inline-flex;
    gap: 4px;
  }
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: grid;
    place-items: center;
    padding: var(--space-3);
    background: var(--scrim);
  }
  .lightbox figure {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 100%;
    max-height: 100%;
  }
  .lightbox img {
    max-width: 90vw;
    max-height: 80vh;
    object-fit: contain;
    border-radius: var(--radius-sm);
  }
  .lightbox figcaption {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    justify-content: space-between;
    font-size: var(--text-xs);
    color: var(--fg);
  }
</style>
