<!--
  Control's Library (Orbit Fleet redesign step 9.7, board MCViews): what is
  on the fleet's hosts that a person may want back, in one list. Session
  outputs and downloads (`downloads.ts`), the files a person put on a host
  by Upload… or with a prompt (`library.ts`), and the repos the sessions work
  in, per host. Upload… puts files beside the session in focus.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { selectedSession } from './selection';
  import { fmtSize, saveDownload } from './downloads';
  import { timeAgo } from './session_status';
  import {
    ENTRY_LABEL,
    libraryEntryList,
    matchesFilter,
    refreshLibrary,
    removeLibraryItem,
    uploadToSession,
    type LibraryEntry,
    type LibraryFilter,
  } from './library';
  import type { IpcError } from './result';
  import LoadError from './states/LoadError.svelte';

  const FILTERS: readonly { id: LibraryFilter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'files', label: 'Downloads' },
    { id: 'uploads', label: 'Uploads' },
    { id: 'repos', label: 'Repos' },
  ];

  let filter = $state<LibraryFilter>('all');
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** The Library's own read failed: said as a failure, not as empty. */
  let loadError = $state<IpcError | null>(null);
  let loading = $state(false);

  async function load() {
    loading = true;
    loadError = await refreshLibrary();
    loading = false;
  }

  const shown = $derived($libraryEntryList.filter((e) => matchesFilter(e, filter)));
  const hosts = $derived([...new Set(shown.map((e) => e.host))]);

  onMount(() => {
    void load();
  });

  async function upload() {
    const s = $selectedSession;
    if (!s || busy) return;
    busy = true;
    error = null;
    const r = await uploadToSession(s);
    busy = false;
    if (r && !r.ok) error = r.error.message;
  }

  async function forget(e: LibraryEntry) {
    if (!e.item) return;
    const r = await removeLibraryItem(e.item.id);
    if (!r.ok) error = r.error.message;
  }

  async function save(e: LibraryEntry) {
    if (!e.download) return;
    await saveDownload(e.download.id);
  }
</script>

<div class="library" data-testid="library-view">
  <div class="bar">
    <div class="filters" role="group" aria-label="Show">
      {#each FILTERS as f (f.id)}
        <button
          type="button"
          class="chip"
          aria-pressed={filter === f.id}
          data-testid="library-filter-{f.id}"
          onclick={() => (filter = f.id)}>{f.label}</button
        >
      {/each}
    </div>
    <span class="grow"></span>
    <button
      type="button"
      class="upload"
      data-testid="library-upload"
      disabled={!$selectedSession || busy}
      title={$selectedSession
        ? `Put files beside ${$selectedSession.tmux_name} on ${$selectedSession.host_alias}`
        : 'Pick a session first: uploads go beside the session in focus'}
      onclick={upload}>{busy ? 'Uploading…' : 'Upload…'}</button
    >
  </div>
  {#if error}<p class="error" role="alert" data-testid="library-error">{error}</p>{/if}

  {#if loadError && shown.length === 0}
    <LoadError title="Couldn't load the library" error={loadError} onretry={load} retrying={loading} testid="library-load-error" />
  {:else if shown.length === 0}
    <p class="empty" data-testid="library-empty">
      {filter === 'repos' ? 'No repos yet: they appear once a session works in one.' : 'Nothing here yet. Files a session sends, and files you upload, land here.'}
    </p>
  {:else}
    {#each hosts as host (host)}
      <section class="host" aria-label={host}>
        <h3>{host}</h3>
        <ul>
          {#each shown.filter((e) => e.host === host) as e (e.key)}
            <li class="row" data-testid="library-row" data-kind={e.kind}>
              <span class="name" title={e.detail}>{e.name}</span>
              <span class="meta"
                >{ENTRY_LABEL[e.kind]}{#if e.session}{' '}· {e.session}{/if}{#if e.size != null}{' '}· {fmtSize(e.size)}{/if}{#if e.at}{' '}· {timeAgo(e.at)}{/if}</span
              >
              {#if e.download && e.download.state === 'ready'}
                <button type="button" class="act" aria-label="Save {e.name}…" onclick={() => save(e)}>Save…</button>
              {:else if e.item}
                <button type="button" class="act" aria-label="Remove {e.name} from the Library" onclick={() => forget(e)}
                  >Remove</button
                >
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</div>

<style>
  .library {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .filters {
    display: flex;
    gap: 2px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1 1 auto;
  }
  .chip,
  .upload,
  .act {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 8px;
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .chip[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--fg);
  }
  .upload {
    color: var(--fg);
  }
  .upload:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .chip:focus-visible,
  .upload:focus-visible,
  .act:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .host h3 {
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
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 2px var(--space-2);
    padding: var(--space-1) var(--space-3);
    border-bottom: 1px solid var(--border);
    font-size: var(--text-sm);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    grid-column: 1;
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .act {
    grid-column: 2;
    grid-row: 1 / span 2;
    align-self: center;
  }
  .empty,
  .error {
    margin: 0;
    padding: var(--space-3);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .error {
    color: var(--status-failed);
  }
</style>
