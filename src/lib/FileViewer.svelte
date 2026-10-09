<script lang="ts">
  import type { SessionRow } from './sessions';
  import {
    repoBlame,
    repoDiff,
    repoFile,
    repoRangeDiff,
    blameGutter,
    canBlame,
    hasDiff,
    type FileBlame,
    type FileContent,
    type FileDiff,
    type DiffRange,
  } from './files';
  import { timeAgo } from './session_status';
  import { repoCommitDiff } from './history';
  import DiffView from './DiffView.svelte';
  import { highlight, langForPath } from './highlight';
  import { sendFile } from './downloads';
  import { sessionView } from './prefs';
  import { copyText } from './clipboard';
  import { insertIntoComposer } from './conversation';
  import { goTo } from './destination';
  import { accessOf } from './access';
  import { editorBlockedReason, openSessionInEditor } from './editor';

  let {
    session,
    path,
    status,
    reloadKey,
    commit = null,
    range = null,
    focusLine = null,
  }: {
    session: SessionRow;
    path: string | null;
    status: string | undefined;
    /** Bumped by the panel on Refresh — invalidates the viewer caches. */
    reloadKey: number;
    /** When set, show this file's diff *within* the commit, not the worktree. */
    commit?: string | null;
    /** When set, show this file's diff over a committed range of the
     *  branch (not pushed, or against its base), not the worktree. */
    range?: DiffRange | null;
    /** 1-based line to show and highlight in the File view (a path clicked
     *  in the Conversation tab); null for none. */
    focusLine?: number | null;
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
  const cacheKey = (sid: number, p: string) => `${sid}:${reloadKey}:${commit ?? range ?? 'wt'}:${p}`;

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
  // A committed range row counts as another file: the same path picked
  // under "not pushed" after the worktree row reopens on its Diff.
  let lastPath: string | null = null;
  $effect(() => {
    const key = path === null ? null : `${range ?? 'wt'}:${path}`;
    if (key !== lastPath) {
      lastPath = key;
      view = commit || range ? 'diff' : canDiff ? 'diff' : 'file';
    }
  });

  // A requested line lives in the File view; once the content is in, bring
  // the row into view. Keyed on the line and the loaded file, so a later
  // manual scroll is never undone by an unrelated re-render.
  let fileEl: HTMLDivElement | undefined = $state();
  $effect(() => {
    if (focusLine !== null && path && commit === null) view = 'file';
  });
  $effect(() => {
    const line = focusLine;
    const loaded = file;
    if (line === null || commit !== null || !loaded || !fileEl) return;
    const row = fileEl.children[line - 1] as HTMLElement | undefined;
    row?.scrollIntoView?.({ block: 'center' });
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
      const r = commit
        ? await repoCommitDiff(sid, commit, p)
        : range
          ? await repoRangeDiff(sid, p, range)
          : await repoDiff(sid, p);
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

  // Blame: a gutter beside the File view, on for every file until turned
  // off. Fetched separately from the content (it is slower), cached the same
  // way, and never shown inside a commit view.
  let blameOn = $state(false);
  let blame = $state<FileBlame | null>(null);
  let blameError = $state<string | null>(null);
  const blameCache = new Map<string, FileBlame>();
  let blameSeq = 0;
  const blameable = $derived(commit === null && canBlame(status));
  const showBlame = $derived(blameOn && blameable && view === 'file');

  $effect(() => {
    const p = path;
    reloadKey; // tracked — a Refresh re-fetches
    if (!showBlame || !p) {
      blame = null;
      blameError = null;
      return;
    }
    void loadBlame(p);
  });

  async function loadBlame(p: string): Promise<void> {
    const sid = session.id;
    const token = ++blameSeq;
    blameError = null;
    const k = cacheKey(sid, p);
    const cached = blameCache.get(k);
    if (cached) {
      blame = cached;
      return;
    }
    blame = null;
    const r = await repoBlame(sid, p);
    if (token !== blameSeq) return;
    if (r.ok) {
      cachePut(blameCache, k, r.value);
      blame = r.value;
    } else {
      blameError = r.error.message;
    }
  }

  function toggleBlame(): void {
    blameOn = !blameOn;
    if (blameOn) view = 'file';
  }

  // Tokenised file body, one token row per line. Keyed off the path's
  // extension; binary files never reach here.
  const hlLines = $derived(
    file && !file.binary && !file.is_dir
      ? highlight(file.content, langForPath(path))
      : [],
  );
  const gutter = $derived(showBlame && blame ? blameGutter(blame.hunks, hlLines.length) : []);

  // The viewer actions (redesign step 5.6, Files board): the
  // path to the clipboard, the path into this session's composer, and the
  // worktree in VS Code (the 5.5 command, with its reasons).
  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(copiedTimer));
  async function copyPath(p: string): Promise<void> {
    if (!(await copyText(p))) return;
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = false), 1_500);
  }
  function mention(p: string): void {
    insertIntoComposer(session.id, `@${p}`);
    sessionView.set('conversation');
    goTo('session');
  }
  const editorBlocked = $derived(editorBlockedReason(session, $accessOf(session)));
</script>

<div class="viewer" data-testid="file-viewer">
  {#if !path}
    <p class="empty">Select a file to view it.</p>
  {:else}
    {@const p = path}
    <header class="bar">
      <span class="path" title={path}>{path}</span>
      <div class="toggle">
        <button
          class:active={view === 'diff'}
          disabled={!canDiff}
          title={canDiff ? 'Show diff' : 'No diff — file is untracked'}
          onclick={() => (view = 'diff')}>Diff</button
        >
        <button
          class:active={view === 'file'}
          disabled={commit !== null}
          title={commit !== null ? 'Viewing a commit — working-tree file not shown' : 'Show file'}
          onclick={() => (view = 'file')}>File</button
        >
      </div>
      {#if commit === null}
        <button
          class="blame-toggle"
          class:active={showBlame}
          aria-pressed={showBlame}
          data-testid="blame-toggle"
          disabled={!blameable}
          title={blameable ? 'Show who last changed each line' : 'No blame — the file is not in any commit yet'}
          onclick={toggleBlame}>Blame</button
        >
        <button
          class="send"
          data-testid="send-to-downloads"
          title="Copy this file to Downloads, for your phone and this window"
          onclick={() => path && void sendFile(session.id, path)}>⤓ Send to downloads</button
        >
      {/if}
        <div class="actions" role="group" aria-label="File actions">
          <button type="button" class="act" data-testid="viewer-copy-path" onclick={() => void copyPath(p)}
            >{copied ? 'Copied' : 'Copy path'}</button
          >
          <button
            type="button"
            class="act"
            data-testid="viewer-mention"
            title="Add @{p} to this session's message"
            onclick={() => mention(p)}>Mention in chat</button
          >
          <button
            type="button"
            class="act"
            data-testid="viewer-open-editor"
            disabled={editorBlocked !== null}
            title={editorBlocked ?? 'Open this worktree in VS Code'}
            onclick={() => void openSessionInEditor(session)}>Open in VS Code</button
          >
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
        {#if file.is_dir}
          <p class="hint">This entry is a directory — no file content to show.</p>
        {:else if file.binary}
          <p class="hint">Binary file — content not shown.</p>
        {:else}
          {#if file.truncated}
            <p class="hint">File truncated (over 512 KiB).</p>
          {/if}
          {#if showBlame && blameError}
            <p class="hint err" data-testid="blame-error">Blame: {blameError}</p>
          {:else if showBlame && blame?.truncated}
            <p class="hint">Blame stops at line {blame.hunks.reduce((n, h) => Math.max(n, h.start + h.lines - 1), 0)}.</p>
          {/if}
          <div class="file" bind:this={fileEl}>
            {#each hlLines as toks, i}
              <div class="frow" class:focus={commit === null && focusLine === i + 1} data-testid={commit === null && focusLine === i + 1 ? 'file-focus-row' : undefined}>
                {#if showBlame}
                  {@const g = gutter[i]}
                  <span
                    class="blame"
                    class:first={g?.first}
                    class:uncommitted={g?.hunk.uncommitted}
                    title={g ? (g.hunk.uncommitted ? 'Not committed yet' : `${g.hunk.hash.slice(0, 8)} · ${g.hunk.author}\n${g.hunk.summary}`) : ''}
                    data-testid={g?.first ? 'blame-label' : undefined}
                    >{#if g?.first}{g.hunk.uncommitted ? 'Not committed' : `${g.hunk.hash.slice(0, 7)} ${g.hunk.author} · ${timeAgo(g.hunk.time)}`}{/if}</span
                  >
                {/if}
                <span class="fno">{i + 1}</span><span class="ftext"
                  >{#each toks as t}{#if t.cls === 'txt'}{t.text}{:else}<span class={t.cls}>{t.text}</span>{/if}{:else}&nbsp;{/each}</span
                >
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
    font-size: var(--text-xs);
    padding: 0.6rem 0.8rem;
    margin: 0;
  }
  .hint.err {
    color: var(--danger);
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
    font-family: var(--mono);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .actions {
    display: flex;
    gap: 0.3rem;
    margin-left: auto;
  }
  .act {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.15rem 0.55rem;
    white-space: nowrap;
  }
  .act:hover:not(:disabled) {
    color: var(--fg);
  }
  .act:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .send {
    margin-left: 0.5rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.15rem 0.55rem;
    white-space: nowrap;
  }
  .send:hover {
    color: var(--fg);
  }
  .blame-toggle {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.15rem 0.55rem;
  }
  .blame-toggle.active {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-pane));
    color: var(--fg);
    border-color: var(--accent);
  }
  .blame-toggle:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .blame {
    flex: 0 0 auto;
    width: 22ch;
    padding: 0 0.6em;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-muted);
    border-right: 1px solid var(--border);
    font-size: var(--text-2xs);
    user-select: none;
  }
  .blame.first {
    box-shadow: inset 0 1px 0 var(--border);
  }
  .blame.uncommitted {
    color: var(--accent);
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
    font-size: var(--text-2xs);
    padding: 0.15rem 0.55rem;
  }
  .toggle button:first-child {
    border-radius: var(--radius-sm) 0 0 var(--radius-sm);
  }
  .toggle button:last-child {
    border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
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
    font-family: var(--mono);
    font-size: var(--text-2xs);
    line-height: 1.5;
    white-space: pre;
  }
  .frow {
    display: flex;
    align-items: baseline;
  }
  .frow.focus {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    box-shadow: inset 3px 0 0 var(--accent);
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
  /* Syntax token colours. All are theme variables (see app.css) so they
     keep their contrast on both the light and dark themes. */
  .ftext .com {
    color: var(--fg-muted);
    font-style: italic;
  }
  .ftext .kw {
    color: var(--syn-kw);
  }
  .ftext .str {
    color: var(--syn-str);
  }
  .ftext .num {
    color: var(--syn-num);
  }
  /* Markdown: headings and code spans/fences. */
  .ftext .head {
    color: var(--accent);
    font-weight: 700;
  }
  .ftext .code {
    color: var(--syn-code);
  }
</style>
