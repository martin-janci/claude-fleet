<script lang="ts">
  import { tablistKeys } from './tablist_keys';
  import { onMount } from 'svelte';
  import Modal from './Modal.svelte';
  import NotificationList from './NotificationList.svelte';
  import { unreadNotices } from './notifications';
  import {
    downloads,
    downloadBudget,
    finished,
    loadDownloads,
    saveDownload,
    removeDownload,
    retryDownload,
    revealLabel,
    revealSaved,
    savedTo,
    transferFraction,
    transferText,
    fmtSize,
    type Download,
  } from './downloads';
  import Loader from './Loader.svelte';
  import LoadError from './states/LoadError.svelte';
  import type { IpcError } from './result';
  import { transferLoader } from './transfer_loader';

  // The notification centre and the Downloads list, one sheet with a tab
  // each (canvas board Toasts). Downloads are files a session sent from its
  // host; the list is the store App keeps fresh on `download:changed`, and
  // opening re-reads it once. Notifications are this window's toasts.
  let { onclose, tab: initialTab = 'downloads' }: { onclose: () => void; tab?: 'downloads' | 'notifications' } = $props();

  // svelte-ignore state_referenced_locally
  let tab = $state(initialTab);
  let error = $state<IpcError | null>(null);
  let retrying = $state(false);
  let busy = $state<number | null>(null);
  const done = $derived(finished($downloads));

  async function load() {
    retrying = true;
    const r = await loadDownloads();
    retrying = false;
    error = r.ok ? null : r.error;
  }

  onMount(() => {
    void load();
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

  async function retry(d: Download) {
    busy = d.id;
    try {
      await retryDownload(d);
    } finally {
      busy = null;
    }
  }

  async function clearFinished() {
    for (const d of done) await removeDownload(d.id);
  }
</script>

<Modal title={tab === 'downloads' ? 'Downloads' : 'Notifications'} {onclose} width="560px" testid="downloads-sheet">
  <div class="tabs" role="tablist" aria-label="Downloads and notifications" use:tablistKeys>
    <button type="button" role="tab" aria-selected={tab === 'downloads'} data-testid="tab-downloads" onclick={() => (tab = 'downloads')}>Downloads</button>
    <button type="button" role="tab" aria-selected={tab === 'notifications'} data-testid="tab-notifications" onclick={() => (tab = 'notifications')}
      >Notifications{$unreadNotices > 0 ? ` (${$unreadNotices})` : ''}</button
    >
  </div>
  {#if tab === 'notifications'}
    <NotificationList />
  {:else if error}
    <LoadError title="Couldn't load downloads" {error} onretry={load} {retrying} testid="downloads-error" />
  {:else if $downloads.length === 0}
    <p class="hint">
      Nothing yet. Ask a session's Claude to send you a file and it lands here, or use
      <em>Send to downloads</em> on a file in the Files tab.
    </p>
  {:else}
    <div class="bar">
      <button
        type="button"
        data-testid="downloads-clear-finished"
        title="Remove the copies of saved files and failed copies"
        disabled={done.length === 0}
        onclick={clearFinished}>Clear finished</button
      >
    </div>
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
            {#if d.state === 'fetching'}
              {@const l = transferLoader(transferFraction(d))}
              <!-- Step 10.10: a known size always shows the Progress ring,
                   an unknown one Data rain. -->
              <div class="transfer">
                {#if l.name === 'progress-ring'}
                  <span
                    class="ring"
                    role="progressbar"
                    aria-label="Copying {d.name}"
                    aria-valuemin="0"
                    aria-valuemax="100"
                    aria-valuenow={Math.round(l.value * 100)}
                  >
                    <Loader name="progress-ring" size={20} value={l.value} testid="download-ring" />
                  </span>
                {:else}
                  <Loader name="data-rain" size={20} stage={false} testid="download-rain" />
                {/if}
                <span class="state" data-testid="download-progress">{transferText(d)}</span>
              </div>
            {/if}
          </div>
          <div class="actions">
            {#if d.state === 'failed'}
              <button data-testid="download-retry" disabled={busy === d.id} onclick={() => retry(d)}>Retry</button>
            {:else if d.state === 'ready' && $savedTo.has(d.id)}
              <button data-testid="download-reveal" onclick={() => revealSaved(d.id)}>{revealLabel()}</button>
            {/if}
            {#if d.state === 'ready'}
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
  .tabs {
    display: flex;
    gap: 0.25rem;
    margin-bottom: 0.5rem;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    background: transparent;
    border: 0;
    border-bottom: 2px solid transparent;
    color: var(--fg-muted);
    cursor: pointer;
    font: inherit;
    font-size: var(--text-xs);
    padding: 0.3rem 0.6rem;
  }
  .tabs button[aria-selected='true'] {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .bar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 0.3rem;
  }
  .bar button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.2rem 0.55rem;
  }
  .bar button:disabled { opacity: 0.5; cursor: default; }
  .transfer {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0.2rem 0 0.1rem;
  }
  .ring {
    display: inline-flex;
  }
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
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err {
    color: var(--danger);
  }
  .actions {
    display: flex;
    gap: 0.35rem;
    flex: none;
  }
  .actions button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.2rem 0.55rem;
  }
  .actions button:hover {
    color: var(--fg);
  }
  .actions button.fresh {
    color: var(--fg);
    border-color: var(--accent);
  }
  .hint {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
