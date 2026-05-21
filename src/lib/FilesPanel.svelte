<script lang="ts">
  import type { SessionRow } from './sessions';
  import {
    repoChanges,
    repoTree,
    type ChangedFile,
    type RepoTree,
  } from './files';
  import { readPref, writePref } from './prefs';
  import FileList from './FileList.svelte';
  import FileViewer from './FileViewer.svelte';
  import Resizer from './Resizer.svelte';

  let { session }: { session: SessionRow } = $props();

  const isNumber = (v: unknown): v is number => typeof v === 'number';

  let mode = $state<'changes' | 'tree'>('changes');
  let changes = $state<ChangedFile[]>([]);
  let tree = $state<RepoTree | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selectedPath = $state<string | null>(null);
  let treeLoaded = false;
  // Bumped on Refresh — invalidates FileViewer's content/diff caches.
  let reloadKey = $state(0);

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
    void loadChanges();
  });

  async function loadChanges(): Promise<void> {
    loading = true;
    error = null;
    const r = await repoChanges(session.id);
    loading = false;
    if (r.ok) changes = r.value;
    else error = r.error.message;
  }

  async function loadTree(): Promise<void> {
    loading = true;
    error = null;
    const r = await repoTree(session.id);
    loading = false;
    if (r.ok) {
      tree = r.value;
      treeLoaded = true;
    } else {
      error = r.error.message;
    }
  }

  function onMode(m: 'changes' | 'tree'): void {
    mode = m;
    error = null;
    if (m === 'tree' && !treeLoaded) void loadTree();
  }

  function onRefresh(): void {
    if (mode === 'changes') void loadChanges();
    else void loadTree();
    reloadKey++;
  }

  function onSelect(path: string): void {
    selectedPath = path;
  }

  function onResize(delta: number): void {
    listPx = Math.max(160, Math.min(560, listPx + delta));
  }
</script>

<div class="files-panel" data-testid="files-panel" style="--list-px: {listPx}px">
  <div class="list-col">
    <FileList
      {mode}
      {changes}
      {tree}
      {loading}
      {error}
      {selectedPath}
      {onSelect}
      {onMode}
      {onRefresh}
    />
  </div>
  <Resizer id="files-list" onresize={onResize} />
  <div class="viewer-col">
    <FileViewer {session} path={selectedPath} status={selectedStatus} {reloadKey} />
  </div>
</div>

<style>
  .files-panel {
    display: grid;
    grid-template-columns: var(--list-px) 4px 1fr;
    height: 100%;
    width: 100%;
    background: var(--bg);
    overflow: hidden;
  }
  .list-col,
  .viewer-col {
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
</style>
