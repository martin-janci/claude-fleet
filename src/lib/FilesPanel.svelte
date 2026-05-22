<script lang="ts">
  import type { SessionRow } from './sessions';
  import {
    repoChanges,
    repoTree,
    type ChangedFile,
    type RepoTree,
  } from './files';
  import { repoLog, repoCommit, type Commit, type CommitDetail } from './history';
  import { readPref, writePref } from './prefs';
  import FileList from './FileList.svelte';
  import FileViewer from './FileViewer.svelte';
  import Resizer from './Resizer.svelte';
  import CommitGraph from './CommitGraph.svelte';

  let { session }: { session: SessionRow } = $props();

  const isNumber = (v: unknown): v is number => typeof v === 'number';

  let mode = $state<'changes' | 'tree' | 'history' | 'branches'>('changes');
  let changes = $state<ChangedFile[]>([]);
  let tree = $state<RepoTree | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selectedPath = $state<string | null>(null);
  let treeLoaded = false;
  // Bumped on Refresh — invalidates FileViewer's content/diff caches.
  let reloadKey = $state(0);

  // History state
  let commits = $state<Commit[]>([]);
  let historyLoaded = false;
  let logSkip = 0;
  let allBranches = $state(true);
  let openCommit = $state<CommitDetail | null>(null);

  let listPx = $state(readPref('layout.files-list', 280, isNumber));
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const px = listPx;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => writePref('layout.files-list', px), 200);
    return () => clearTimeout(saveTimer);
  });

  const selectedStatus = $derived(
    selectedPath ? changes.find((c) => c.path === selectedPath)?.status : undefined,
  );

  // Reload from scratch whenever the selected session changes. The panel is
  // remounted on each entry into files mode, so this also covers first load.
  let lastSessionId: number | null = null;
  $effect(() => {
    const id = session.id;
    if (id === lastSessionId) return;
    lastSessionId = id;
    mode = 'changes';
    changes = [];
    tree = null;
    treeLoaded = false;
    selectedPath = null;
    commits = [];
    historyLoaded = false;
    openCommit = null;
    void loadChanges();
  });

  async function loadChanges(): Promise<void> {
    const sid = session.id;
    loading = true;
    error = null;
    const r = await repoChanges(sid);
    // The user switched sessions while this was in flight — the session
    // effect has already started a fresh load; drop this stale result.
    if (sid !== session.id) return;
    loading = false;
    if (r.ok) changes = r.value;
    else error = r.error.message;
  }

  async function loadTree(): Promise<void> {
    const sid = session.id;
    loading = true;
    error = null;
    const r = await repoTree(sid);
    if (sid !== session.id) return;
    loading = false;
    if (r.ok) {
      tree = r.value;
      treeLoaded = true;
    } else {
      error = r.error.message;
    }
  }

  async function loadHistory(reset = true): Promise<void> {
    const sid = session.id;
    loading = true;
    error = null;
    if (reset) { logSkip = 0; commits = []; }
    const r = await repoLog(sid, { all: allBranches, skip: logSkip });
    if (sid !== session.id) return;
    loading = false;
    if (r.ok) {
      commits = reset ? r.value : [...commits, ...r.value];
      historyLoaded = true;
      logSkip = commits.length;
    } else {
      error = r.error.message;
    }
  }

  async function openCommitDetail(hash: string): Promise<void> {
    const sid = session.id;
    const r = await repoCommit(sid, hash);
    if (sid !== session.id) return;
    if (r.ok) { openCommit = r.value; selectedPath = r.value.files[0]?.path ?? null; }
    else error = r.error.message;
  }

  function backToGraph(): void { openCommit = null; selectedPath = null; }

  function loadBranches(): void {} // replaced in Task 11
  function promptCreateBranch(_hash: string | null): void {} // replaced in Task 13
  function confirmCheckoutCommit(_hash: string): void {} // replaced in Task 13

  function onMode(m: typeof mode): void {
    mode = m;
    error = null;
    openCommit = null;
    if (m === 'tree' && !treeLoaded) void loadTree();
    if (m === 'history' && !historyLoaded) void loadHistory();
    if (m === 'branches') void loadBranches();
  }

  function onRefresh(): void {
    if (mode === 'changes') void loadChanges();
    else if (mode === 'tree') void loadTree();
    else if (mode === 'history') void loadHistory();
    else void loadBranches();
    reloadKey++;
  }

  function onSelect(path: string): void {
    selectedPath = path;
  }

  function onResize(delta: number): void {
    listPx = Math.max(160, Math.min(560, listPx + delta));
  }
</script>

<div class="panel-wrap">
  <!-- Shared mode toggle header, always visible -->
  <div class="panel-header">
    <div class="modes">
      <button class:active={mode === 'changes'} onclick={() => onMode('changes')}>Changed</button>
      <button class:active={mode === 'tree'} onclick={() => onMode('tree')}>All files</button>
      <button class:active={mode === 'history'} onclick={() => onMode('history')}>History</button>
      <button class:active={mode === 'branches'} onclick={() => onMode('branches')}>Branches</button>
    </div>
    <button class="refresh" title="Refresh" aria-label="Refresh" onclick={onRefresh}>↻</button>
  </div>

  <!-- Mode-aware body -->
  {#if mode === 'history' && !openCommit}
    <div class="full-col" data-testid="history-view">
      <div class="hbar">
        <label><input type="checkbox" bind:checked={allBranches} onchange={() => loadHistory()} /> All branches</label>
      </div>
      <div class="hscroll">
        {#if loading && commits.length === 0}
          <p class="hint">Loading…</p>
        {:else if error}
          <p class="hint err">{error}</p>
        {:else}
          <CommitGraph
            {commits}
            selected={null}
            onSelect={(h) => openCommitDetail(h)}
            onCreateBranch={(h) => promptCreateBranch(h)}
            onCheckoutCommit={(h) => confirmCheckoutCommit(h)}
          />
          {#if commits.length > 0}
            <button class="more" onclick={() => loadHistory(false)}>Load more</button>
          {/if}
        {/if}
      </div>
    </div>
  {:else if mode === 'branches'}
    <div class="full-col" data-testid="branches-view"></div>
  {:else}
    <div class="files-panel" data-testid="files-panel" style="--list-px: {listPx}px">
      <div class="list-col">
        {#if openCommit}
          <div class="commit-head">
            <button class="back" onclick={backToGraph}>← Back to graph</button>
            <div class="csub">{openCommit.subject}</div>
            <div class="cmeta">{openCommit.author} · {openCommit.hash.slice(0, 8)}</div>
          </div>
          <FileList
            mode="changes"
            changes={openCommit.files}
            tree={null}
            {loading}
            {error}
            {selectedPath}
            {onSelect}
          />
        {:else}
          <FileList {mode} {changes} {tree} {loading} {error} {selectedPath} {onSelect} />
        {/if}
      </div>
      <Resizer id="files-list" onresize={onResize} />
      <div class="viewer-col">
        <FileViewer {session} path={selectedPath} status={selectedStatus} {reloadKey} commit={openCommit?.hash ?? null} />
      </div>
    </div>
  {/if}
</div>

<style>
  .panel-wrap {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    background: var(--bg);
    overflow: hidden;
  }
  .panel-header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 0.5rem;
    flex: 0 0 auto;
    border-bottom: 1px solid var(--border);
  }
  .modes {
    display: flex;
    flex: 1 1 auto;
  }
  .modes button {
    flex: 1 1 0;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0.2rem 0.4rem;
  }
  .modes button:first-child {
    border-radius: 4px 0 0 4px;
  }
  .modes button:not(:first-child) {
    border-left: none;
  }
  .modes button:last-child {
    border-radius: 0 4px 4px 0;
  }
  .modes button.active {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-pane));
    color: var(--fg);
    border-color: var(--accent);
  }
  .refresh {
    flex: 0 0 auto;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.85rem;
    width: 1.7rem;
    height: 1.6rem;
    padding: 0;
  }
  .refresh:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
  .files-panel {
    display: grid;
    grid-template-columns: var(--list-px) 4px 1fr;
    flex: 1 1 auto;
    min-height: 0;
    width: 100%;
    overflow: hidden;
  }
  .list-col {
    display: flex;
    flex-direction: column;
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
  .viewer-col {
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
  .full-col {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 0;
    overflow: hidden;
  }
  .hbar {
    padding: 0.3rem 0.6rem;
    font-size: 0.78rem;
    flex: 0 0 auto;
    border-bottom: 1px solid var(--border);
    color: var(--fg-muted);
  }
  .hbar label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    cursor: pointer;
  }
  .hscroll {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 0.82rem;
    padding: 0.5rem 0.7rem;
    margin: 0;
  }
  .hint.err {
    color: #e64a4a;
  }
  .more {
    display: block;
    width: 100%;
    background: transparent;
    border: none;
    border-top: 1px solid var(--border);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.78rem;
    padding: 0.4rem 0.7rem;
    text-align: center;
  }
  .more:hover {
    color: var(--fg);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .commit-head {
    padding: 0.4rem 0.5rem;
    border-bottom: 1px solid var(--border);
    flex: 0 0 auto;
  }
  .back {
    background: transparent;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0;
    margin-bottom: 0.2rem;
  }
  .back:hover {
    color: var(--fg);
  }
  .csub {
    font-size: 0.78rem;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cmeta {
    font-size: 0.7rem;
    color: var(--fg-muted);
    margin-top: 0.1rem;
  }
</style>
