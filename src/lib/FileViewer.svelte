<script lang="ts">
  import type { SessionRow } from './sessions';
  import {
    repoDiff,
    repoFile,
    hasDiff,
    type FileContent,
    type FileDiff,
  } from './files';
  import DiffView from './DiffView.svelte';

  let {
    session,
    path,
    status,
    reloadKey,
  }: {
    session: SessionRow;
    path: string | null;
    status: string | undefined;
    /** Bumped by the panel on Refresh — invalidates the viewer caches. */
    reloadKey: number;
  } = $props();

  type View = 'diff' | 'file';
  let view = $state<View>('diff');
  let loading = $state(false);
  let error = $state<string | null>(null);
  let diff = $state<FileDiff | null>(null);
  let file = $state<FileContent | null>(null);

  // Cache results so re-selecting a file (or flipping Diff/File and back) is
  // instant. Keyed by session + reloadKey + path, so a Refresh or a switch to
  // a different session never serves stale content.
  const diffCache = new Map<string, FileDiff>();
  const fileCache = new Map<string, FileContent>();
  const cacheKey = (sid: number, p: string) => `${sid}:${reloadKey}:${p}`;

  // Cap each cache: a long session-hopping run could otherwise pin many
  // large (up to 512 KiB) file bodies in memory for the panel's lifetime.
  // A Map preserves insertion order, so the first key is the oldest.
  const MAX_CACHE_ENTRIES = 40;
  function cachePut<T>(cache: Map<string, T>, k: string, v: T): void {
    cache.set(k, v);
    while (cache.size > MAX_CACHE_ENTRIES) {
      const oldest = cache.keys().next().value;
      if (oldest === undefined) break;
      cache.delete(oldest);
    }
  }

  // Monotonic token: every `load()` claims one, and only the most recent
  // claim may mutate `loading`/`diff`/`file`/`error`. A fetch that was
  // superseded (newer selection, view flip, or session switch) returns
  // silently — so a slow stale fetch can never clear a fresh load's spinner
  // or overwrite its result.
  let loadSeq = 0;

  const canDiff = $derived(hasDiff(status));

  // When the selected file changes, pick a sensible default view: a changed
  // (non-untracked) file opens on its Diff; anything else on its content.
  let lastPath: string | null = null;
  $effect(() => {
    if (path !== lastPath) {
      lastPath = path;
      view = canDiff ? 'diff' : 'file';
    }
  });

  // Load whatever the current (path, view) needs. Re-runs on reloadKey too,
  // so a Refresh re-fetches. Guarded by the caches.
  $effect(() => {
    const p = path;
    const v = view;
    reloadKey; // tracked — a Refresh invalidates caches via cacheKey()
    if (!p) {
      diff = null;
      file = null;
      error = null;
      return;
    }
    void load(p, v);
  });

  async function load(p: string, v: View): Promise<void> {
    // Pin the session id now: an `await` below may straddle a session switch,
    // and the result must be cached/displayed under the session it came from.
    const sid = session.id;
    const token = ++loadSeq;
    error = null;
    const k = cacheKey(sid, p);
    if (v === 'diff') {
      const cached = diffCache.get(k);
      if (cached) {
        diff = cached;
        return;
      }
      loading = true;
      const r = await repoDiff(sid, p);
      // A newer load (selection, view flip, or session switch) superseded us.
      if (token !== loadSeq) return;
      loading = false;
      if (r.ok) {
        cachePut(diffCache, k, r.value);
        diff = r.value;
      } else {
        error = r.error.message;
      }
    } else {
      const cached = fileCache.get(k);
      if (cached) {
        file = cached;
        return;
      }
      loading = true;
      const r = await repoFile(sid, p);
      if (token !== loadSeq) return;
      loading = false;
      if (r.ok) {
        cachePut(fileCache, k, r.value);
        file = r.value;
      } else {
        error = r.error.message;
      }
    }
  }

  const fileLines = $derived(file ? file.content.split('\n') : []);
</script>

<div class="viewer" data-testid="file-viewer">
  {#if !path}
    <p class="empty">Select a file to view it.</p>
  {:else}
    <header class="bar">
      <span class="path" title={path}>{path}</span>
      <div class="toggle">
        <button
          class:active={view === 'diff'}
          disabled={!canDiff}
          title={canDiff ? 'Show diff' : 'No diff — file is untracked'}
          onclick={() => (view = 'diff')}>Diff</button
        >
        <button class:active={view === 'file'} onclick={() => (view = 'file')}>File</button>
      </div>
    </header>

    <div class="body">
      {#if loading}
        <p class="hint">Loading…</p>
      {:else if error}
        <p class="hint err">{error}</p>
      {:else if view === 'diff' && diff}
        {#if diff.binary}
          <p class="hint">Binary file — diff not shown.</p>
        {:else if diff.diff.trim() === ''}
          <p class="hint">No changes against HEAD.</p>
        {:else}
          {#if diff.truncated}
            <p class="hint">Diff truncated (over 1 MiB).</p>
          {/if}
          <DiffView diff={diff.diff} />
        {/if}
      {:else if view === 'file' && file}
        {#if file.binary}
          <p class="hint">Binary file — content not shown.</p>
        {:else}
          {#if file.truncated}
            <p class="hint">File truncated (over 512 KiB).</p>
          {/if}
          <div class="file">
            {#each fileLines as line, i}
              <div class="frow">
                <span class="fno">{i + 1}</span><span class="ftext">{line || ' '}</span>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  .empty,
  .hint {
    color: var(--fg-muted);
    font-size: 0.85rem;
    padding: 0.6rem 0.8rem;
    margin: 0;
  }
  .hint.err {
    color: #e64a4a;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    flex: 0 0 auto;
  }
  .path {
    flex: 1 1 auto;
    min-width: 0;
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 0.78rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .toggle {
    flex: 0 0 auto;
    display: flex;
  }
  .toggle button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.72rem;
    padding: 0.15rem 0.55rem;
  }
  .toggle button:first-child {
    border-radius: 4px 0 0 4px;
  }
  .toggle button:last-child {
    border-radius: 0 4px 4px 0;
    border-left: none;
  }
  .toggle button.active {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-pane));
    color: var(--fg);
    border-color: var(--accent);
  }
  .toggle button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .body {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
  }
  .file {
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 0.78rem;
    line-height: 1.5;
    white-space: pre;
  }
  .frow {
    display: flex;
    align-items: baseline;
  }
  .fno {
    flex: 0 0 auto;
    width: 3.4em;
    padding: 0 0.6em;
    text-align: right;
    color: var(--fg-muted);
    opacity: 0.6;
    user-select: none;
  }
  .ftext {
    flex: 1 1 auto;
  }
</style>
