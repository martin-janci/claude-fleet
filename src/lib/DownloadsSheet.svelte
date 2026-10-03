<script lang="ts">
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import {
    downloads,
    downloadBudget,
    loadDownloads,
    saveDownload,
    removeDownload,
    fmtSize,
    type Download,
  } from './downloads';

  // Files a session sent from its host (file downloads). The list is the
  // store App keeps fresh on `download:changed`; opening re-reads it once.
  let { onclose }: { onclose: () => void } = $props();

  let error = $state<string | null>(null);
  let busy = $state<number | null>(null);

  onMount(async () => {
    const r = await loadDownloads();
    if (!r.ok) error = r.error.message;
  });

  function age(at: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - at);
    if (s < 60) return 'just now';
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return `${Math.floor(s / 86400)} d ago`;
  }

  async function save(d: Download) {
    busy = d.id;
    try {
      await saveDownload(d.id);
    } finally {
      busy = null;
    }
  }

  async function remove(d: Download) {
    busy = d.id;
    try {
      await removeDownload(d.id);
    } finally {
      busy = null;
    }
  }
</script>

<Modal title="Downloads" {onclose} width="560px" testid="downloads-sheet">
  {#if error}
    <p class="hint err">{error}</p>
  {:else if $downloads.length === 0}
    <p class="hint">
      Nothing yet. A session's Claude sends a file here with <code>send_file</code>, or use
      <em>Send to downloads</em> on a file in the Files tab.
    </p>
  {:else}
    <ul class="list">
      {#each $downloads as d (d.id)}
        <li class="row" data-testid="download-row" data-state={d.state}>
          <div class="main">
            <span class="name" title={d.path}>{d.name}</span>
            <span class="meta">
              {fmtSize(d.size)} · {d.host_alias}{d.session_name ? ` · ${d.session_name}` : ''} ·
              {age(d.at)}
            </span>
            {#if d.note}<span class="note">{d.note}</span>{/if}
            {#if d.state === 'failed'}<span class="note err">{d.error ?? 'copy failed'}</span>{/if}
          </div>
          <div class="actions">
            {#if d.state === 'fetching'}
              <span class="state">copying…</span>
            {:else if d.state === 'ready'}
              <button
                class="primary"
                class:fresh={d.downloaded_at == null}
                disabled={busy === d.id}
                onclick={() => save(d)}>Save…</button
              >
            {/if}
            <button
              disabled={busy === d.id}
              title="Remove the copy"
              aria-label="Remove {d.name}"
              onclick={() => remove(d)}>✕</button
            >
          </div>
        </li>
      {/each}
    </ul>
    {#if $downloadBudget}
      <p class="hint">
        {fmtSize($downloadBudget.total)} of {fmtSize($downloadBudget.max)} kept. Older files are
        removed automatically.
      </p>
    {/if}
  {/if}
</Modal>

<style>
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 60vh;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--border);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta,
  .note,
  .state {
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .err {
    color: var(--danger, #d33);
  }
  .actions {
    display: flex;
    gap: 0.35rem;
    flex: none;
  }
  .actions button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.75rem;
    padding: 0.2rem 0.55rem;
  }
  .actions button:hover {
    color: var(--fg);
  }
  .actions button.fresh {
    color: var(--fg);
    border-color: var(--accent, var(--border));
  }
  .hint {
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
</style>
