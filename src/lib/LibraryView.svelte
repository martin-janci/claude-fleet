<!--
  Control's Library (Orbit Fleet redesign step 9.7, board MCViews): what is
  on the fleet's hosts that a person may want back, in one list. Session
  outputs and downloads (`downloads.ts`), the files a person put on a host
  by Upload… or with a prompt (`library.ts`), and the repos the sessions work
  in, per host. Upload… puts files beside the session in focus.

  Gap plan G3.10: a table per host (Name / Modified / Size, sorted by a
  header press), a grid of tiles instead, Type ▾, and "Link a repo on a
  host", which opens Add project. The board's "Add a Google Drive folder"
  is cut: fleet has no Drive connection to list.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { addProjectRequest } from './app_views';
  import Icon from './kit/Icon.svelte';
  import { readPref, writePref } from './prefs';
  import { selectedSession } from './selection';
  import { fmtSize, saveDownload } from './downloads';
  import { timeAgo } from './session_status';
  import {
    ENTRY_LABEL,
    libraryEntryList,
    matchesFilter,
    nextSort,
    refreshLibrary,
    sortEntries,
    removeLibraryItem,
    uploadToSession,
    type LibraryEntry,
    type LibraryFilter,
    type LibraryLayout,
    type LibrarySort,
    type LibrarySortKey,
  } from './library';
  import type { IpcError } from './result';
  import LoadError from './states/LoadError.svelte';

  const FILTERS: readonly { id: LibraryFilter; label: string }[] = [
    { id: 'all', label: 'All types' },
    { id: 'files', label: 'Downloads' },
    { id: 'uploads', label: 'Uploads' },
    { id: 'repos', label: 'Repos' },
  ];
  const COLUMNS: readonly { key: LibrarySortKey; label: string }[] = [
    { key: 'name', label: 'Name' },
    { key: 'modified', label: 'Modified' },
    { key: 'size', label: 'Size' },
  ];

  const isLayout = (v: unknown): v is LibraryLayout => v === 'list' || v === 'grid';
  let layout = $state<LibraryLayout>(readPref('library.layout', 'list', isLayout));
  function setLayout(l: LibraryLayout) {
    layout = l;
    writePref('library.layout', l);
  }
  let sort = $state<LibrarySort>({ key: 'modified', dir: 'desc' });
  const ariaSort = (k: LibrarySortKey) => (sort.key !== k ? 'none' : sort.dir === 'asc' ? 'ascending' : 'descending');

  /** Link a repo on a host: Add project, which clones or finds it there. */
  function linkRepo() {
    addProjectRequest.set({ cloneUrl: undefined });
  }

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

  const shown = $derived(sortEntries($libraryEntryList.filter((e) => matchesFilter(e, filter)), sort));
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

{#snippet act(e: LibraryEntry)}
  {#if e.download && e.download.state === 'ready'}
    <button type="button" class="act" aria-label="Save {e.name}…" onclick={() => save(e)}>Save…</button>
  {:else if e.item}
    <button type="button" class="act" aria-label="Remove {e.name} from the Library" onclick={() => forget(e)}>Remove</button>
  {/if}
{/snippet}

<div class="library" data-testid="library-view">
  <div class="bar">
    <label class="type">
      <span class="sr">Type</span>
      <select bind:value={filter} data-testid="library-type">
        {#each FILTERS as f (f.id)}<option value={f.id}>{f.label}</option>{/each}
      </select>
    </label>
    <div class="layout" role="group" aria-label="Layout">
      <button type="button" class="chip" aria-pressed={layout === 'list'} title="List" data-testid="library-layout-list" onclick={() => setLayout('list')}
        ><Icon name="list" size={14} /><span class="sr">List</span></button
      >
      <button type="button" class="chip" aria-pressed={layout === 'grid'} title="Grid" data-testid="library-layout-grid" onclick={() => setLayout('grid')}
        ><Icon name="layers" size={14} /><span class="sr">Grid</span></button
      >
    </div>
    <span class="grow"></span>
    <button type="button" class="upload" data-testid="library-link-repo" title="Add a project: clone a repo on a host, or find one already there" onclick={linkRepo}
      >Link a repo…</button
    >
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
      {@const rows = shown.filter((e) => e.host === host)}
      <section class="host" aria-label={host}>
        <h3>{host}</h3>
        {#if layout === 'grid'}
          <ul class="grid" data-testid="library-grid">
            {#each rows as e (e.key)}
              <li class="tile" data-testid="library-row" data-kind={e.kind}>
                <span class="glyph" aria-hidden="true">{e.kind === 'repo' ? '⑂' : '▤'}</span>
                <span class="name" title={e.detail}>{e.name}</span>
                <span class="meta">{ENTRY_LABEL[e.kind]}{#if e.size != null}{' '}· {fmtSize(e.size)}{/if}</span>
                {@render act(e)}
              </li>
            {/each}
          </ul>
        {:else}
          <table class="table" data-testid="library-table">
            <thead>
              <tr>
                {#each COLUMNS as c (c.key)}
                  <th scope="col" aria-sort={ariaSort(c.key)} class="col-{c.key}">
                    <button type="button" class="sort" data-testid="library-sort-{c.key}" onclick={() => (sort = nextSort(sort, c.key))}
                      >{c.label}{#if sort.key === c.key}<span aria-hidden="true">{sort.dir === 'asc' ? ' ▴' : ' ▾'}</span>{/if}</button
                    >
                  </th>
                {/each}
                <th scope="col"><span class="sr">Actions</span></th>
              </tr>
            </thead>
            <tbody>
              {#each rows as e (e.key)}
                <tr class="row" data-testid="library-row" data-kind={e.kind}>
                  <td class="col-name">
                    <span class="name" title={e.detail}>{e.name}</span>
                    <span class="meta">{ENTRY_LABEL[e.kind]}{#if e.session}{' '}· {e.session}{/if}</span>
                  </td>
                  <td class="col-modified meta">{e.at ? timeAgo(e.at) : ''}</td>
                  <td class="col-size meta">{e.size != null ? fmtSize(e.size) : ''}</td>
                  <td class="col-act">{@render act(e)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
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
  .layout {
    display: flex;
    gap: 2px;
  }
  .type select {
    font: inherit;
    font-size: var(--text-xs);
    color: var(--fg);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 2px 4px;
  }
  .table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  .table th {
    text-align: left;
    font-weight: 500;
    padding: 0 var(--space-1);
    border-bottom: 1px solid var(--border);
  }
  .table td {
    padding: var(--space-1);
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  .table .col-name {
    display: flex;
    flex-direction: column;
    max-width: 0;
    width: 100%;
    padding-left: var(--space-3);
  }
  .col-modified,
  .col-size {
    white-space: nowrap;
  }
  .sort {
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 0;
    cursor: pointer;
  }
  .sort:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: var(--space-2);
    padding: 0 var(--space-3) var(--space-2);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    min-width: 0;
  }
  .glyph {
    color: var(--fg-muted);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
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
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--fg-muted);
    font-size: var(--text-xs);
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
