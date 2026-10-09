<script lang="ts" module>
  import type { BranchDiff, ChangedFile, DiffRange, RepoTree } from './files';
  import type { CommitDraft } from './history';
  import type { Result } from './result';
  import DraftField from './DraftField.svelte';
  import { fileIcon, folderIcon } from './fileicons';
  import Icon from './kit/Icon.svelte';
  import Skeleton from './states/Skeleton.svelte';

  interface TreeNode {
    name: string;
    path: string;
    isDir: boolean;
    children: TreeNode[];
  }

  // Build a nested folder tree from a flat, sorted list of file paths.
  // Exported for testing.
  export function buildTree(entries: string[]): TreeNode[] {
    const root: TreeNode = { name: '', path: '', isDir: true, children: [] };
    for (const entry of entries) {
      const parts = entry.split('/');
      let node = root;
      let prefix = '';
      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        prefix = prefix ? `${prefix}/${part}` : part;
        const isDir = i < parts.length - 1;
        let child = node.children.find((c) => c.name === part && c.isDir === isDir);
        if (!child) {
          child = { name: part, path: prefix, isDir, children: [] };
          node.children.push(child);
        }
        node = child;
      }
    }
    sortLevel(root);
    return root.children;
  }

  // Folders before files, each group alphabetical.
  function sortLevel(node: TreeNode): void {
    node.children.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    for (const c of node.children) if (c.isDir) sortLevel(c);
  }

  const BADGE: Record<string, { letter: string; cls: string }> = {
    modified: { letter: 'M', cls: 'b-mod' },
    added: { letter: 'A', cls: 'b-add' },
    deleted: { letter: 'D', cls: 'b-del' },
    renamed: { letter: 'R', cls: 'b-ren' },
    copied: { letter: 'C', cls: 'b-ren' },
    untracked: { letter: '?', cls: 'b-unt' },
    conflict: { letter: '!', cls: 'b-cnf' },
  };
</script>

<script lang="ts">
  let {
    mode,
    changes,
    tree,
    loading,
    error,
    selectedPath,
    onSelect,
    onStageToggle,
    onCommit,
    enableStaging = false,
    writeBlocked = null,
    branch = null,
    selectedRange = null,
    onSelectRange,
    draftCommit,
  }: {
    mode: 'changes' | 'tree' | 'history' | 'branches';
    changes: ChangedFile[];
    tree: RepoTree | null;
    loading: boolean;
    error: string | null;
    selectedPath: string | null;
    onSelect: (path: string, status: string | undefined) => void;
    onStageToggle?: (path: string, staged: boolean) => void;
    onCommit?: (message: string) => void;
    enableStaging?: boolean;
    /** Set when staging/committing has no hub tool (`hubBlock('repo_write', …)`
     *  from `FilesPanel`) — the reason, shown as the checkbox's/button's title. */
    writeBlocked?: string | null;
    /** The committed groups under Changed (redesign step 5.6): null keeps
     *  the flat list of the worktree's own changes. */
    branch?: BranchDiff | null;
    /** The range the selected row came from; null for a worktree row. */
    selectedRange?: DiffRange | null;
    onSelectRange?: (path: string, range: DiffRange, status: string) => void;
    /** The commit message draft (redesign step 5.12): drafts
     *  from the staged changes on the session's host. Absent, the plain
     *  message box. */
    draftCommit?: () => Promise<Result<CommitDraft>>;
  } = $props();

  let filter = $state('');
  let commitMsg = $state('');
  let draft = $state<CommitDraft | null>(null);
  let drafting = $state(false);
  let draftError = $state<string | null>(null);

  async function runDraft(): Promise<void> {
    if (!draftCommit || drafting) return;
    drafting = true;
    draftError = null;
    const r = await draftCommit();
    drafting = false;
    if (r.ok) {
      draft = r.value;
      commitMsg = r.value.message;
    } else {
      draftError = r.error.message;
    }
  }

  function filesFrom(n: number): string {
    return `from ${n} staged file${n === 1 ? '' : 's'}`;
  }
  // Folder expand state — keyed by dir path. Plain object so $state proxies it.
  let expanded = $state<Record<string, boolean>>({});

  const stagedCount = $derived(changes.filter((c) => c.staged).length);

  const statusByPath = $derived(new Map(changes.map((c) => [c.path, c.status])));
  /** The folders a changed file sits in (the FilesTree board's tree marks
   *  them, so a change deep in a closed folder is still seen). */
  const changedDirs = $derived(
    new Set(
      changes.flatMap((c) => {
        const parts = c.path.split('/');
        return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join('/'));
      }),
    ),
  );

  const filterLc = $derived(filter.trim().toLowerCase());

  const filteredChanges = $derived(
    filterLc === ''
      ? changes
      : changes.filter((c) => c.path.toLowerCase().includes(filterLc)),
  );

  const match = (c: ChangedFile) => filterLc === '' || c.path.toLowerCase().includes(filterLc);
  // The committed groups: what is not pushed yet, then the whole branch
  // against its base (folded until opened; it repeats the first group
  // whenever nothing is pushed). An empty group is not shown.
  const groups = $derived(
    branch
      ? [
          {
            range: 'unpushed' as const,
            title: 'In this branch, not pushed',
            files: branch.unpushedFiles,
          },
          {
            range: 'base' as const,
            title: `Against ${branch.base ?? 'base'} · ${branch.aheadOfBase} ahead`,
            files: branch.base ? branch.baseFiles : [],
          },
        ].filter((g) => g.files.length > 0)
      : [],
  );
  let folded = $state<Record<DiffRange, boolean>>({ unpushed: false, base: true });

  const treeNodes = $derived(tree ? buildTree(tree.entries) : []);

  // When a filter is active in tree mode, flatten to matching file paths —
  // walking a deep tree for matches is both slower and worse UX.
  const filteredTreeFlat = $derived(
    tree && filterLc !== ''
      ? tree.entries.filter((e) => e.toLowerCase().includes(filterLc))
      : [],
  );

  function toggle(path: string): void {
    expanded[path] = !expanded[path];
  }
</script>

<div class="list" data-testid="file-list">
  <input
    class="filter"
    type="text"
    placeholder="Filter…"
    bind:value={filter}
    spellcheck="false"
  />

  <div class="rows">
    {#if loading}
      <Skeleton />
    {:else if error}
      <p class="hint err">{error}</p>
    {:else if mode === 'changes'}
      {#if groups.length > 0}
        <h3 class="group" data-testid="group-uncommitted">
          <span>Uncommitted</span><span class="gcount tnum">{changes.length}</span>
        </h3>
      {/if}
      {#if filteredChanges.length === 0}
        <p class="hint">{changes.length === 0 ? (groups.length > 0 ? 'Nothing uncommitted.' : 'No changes.') : 'No matches.'}</p>
      {:else}
        {#each filteredChanges as c (c.path)}
          <div class="row-wrap">
            {#if enableStaging}
              <input
                type="checkbox"
                class="stage"
                checked={c.staged}
                disabled={writeBlocked !== null}
                aria-label="{c.staged ? 'Unstage' : 'Stage'} {c.path}"
                title={writeBlocked ?? (c.staged ? 'Unstage' : 'Stage')}
                onclick={(e) => { e.stopPropagation(); onStageToggle?.(c.path, !c.staged); }}
              />
            {/if}
            <button
              class="row file"
              class:sel={selectedPath === c.path && selectedRange === null}
              onclick={() => onSelect(c.path, c.status)}
              title={c.orig_path ? `${c.orig_path} → ${c.path}` : c.path}
            >
              <span class="badge {BADGE[c.status]?.cls ?? 'b-mod'}"
                >{BADGE[c.status]?.letter ?? '•'}</span
              >
              <span class="ficon"><Icon name={fileIcon(c.path)} size={14} /></span>
              <span class="name">{c.path}</span>
            </button>
          </div>
        {/each}
      {/if}
      {#each groups as g (g.range)}
        {@const shown = g.files.filter(match)}
        <h3 class="group" data-testid="group-{g.range}">
          <button
            type="button"
            class="gtoggle"
            aria-expanded={!folded[g.range]}
            onclick={() => (folded[g.range] = !folded[g.range])}
          >
            <span class="caret" aria-hidden="true">{folded[g.range] ? '▸' : '▾'}</span>
            <span>{g.title}</span>
            <span class="gcount tnum">{g.files.length}</span>
          </button>
        </h3>
        {#if !folded[g.range]}
          {#if shown.length === 0}
            <p class="hint">No matches.</p>
          {/if}
          {#each shown as c (c.path)}
            <button
              class="row file ranged"
              class:sel={selectedPath === c.path && selectedRange === g.range}
              data-testid="range-row"
              onclick={() => onSelectRange?.(c.path, g.range, c.status)}
              title={c.orig_path ? `${c.orig_path} → ${c.path}` : c.path}
            >
              <span class="badge {BADGE[c.status]?.cls ?? 'b-mod'}"
                >{BADGE[c.status]?.letter ?? '•'}</span
              >
              <span class="ficon"><Icon name={fileIcon(c.path)} size={14} /></span>
              <span class="name">{c.path}</span>
            </button>
          {/each}
        {/if}
      {/each}
    {:else if filterLc !== ''}
      <!-- tree mode, filtering → flat matches -->
      {#if filteredTreeFlat.length === 0}
        <p class="hint">No matches.</p>
      {:else}
        {#each filteredTreeFlat as path (path)}
          <button
            class="row file"
            class:sel={selectedPath === path}
            onclick={() => onSelect(path, statusByPath.get(path))}
            title={path}
          >
            <span class="ficon"><Icon name={fileIcon(path)} size={14} /></span>
            <span class="name">{path}</span>
          </button>
        {/each}
      {/if}
    {:else if treeNodes.length === 0}
      <p class="hint">Empty worktree.</p>
    {:else}
      {#each treeNodes as node (node.path)}
        {@render treeRow(node, 0)}
      {/each}
    {/if}
    {#if tree?.truncated && mode === 'tree'}
      <p class="hint">Listing truncated at 20000 files.</p>
    {/if}
  </div>
  {#if enableStaging && mode === 'changes'}
    <div class="commit-footer">
      {#if draftCommit && writeBlocked === null}
        <DraftField
          bind:value={commitMsg}
          label="Commit message"
          model={draft?.model}
          host={draft?.host_alias}
          from={draft ? filesFrom(draft.files) : null}
          busy={drafting}
          rows={3}
          placeholder="Commit message…"
          onregenerate={draft ? runDraft : undefined}
          onclear={() => (draft = null)}
          testid="commit-draft"
        />
        {#if !drafting && commitMsg.trim() === ''}
          <button
            type="button"
            class="draft"
            data-testid="commit-draft-run"
            disabled={stagedCount === 0}
            title={stagedCount === 0 ? 'Stage files first' : 'Write a message from the staged changes'}
            onclick={runDraft}>✎ Draft from staged changes</button
          >
        {/if}
        {#if draftError}
          <span class="draft-error" role="alert" data-testid="commit-draft-error">{draftError}</span>
        {/if}
      {:else}
        <textarea
          bind:value={commitMsg}
          placeholder="Commit message…"
          rows={2}
          disabled={writeBlocked !== null}
          title={writeBlocked ?? ''}
        ></textarea>
      {/if}
      <button
        disabled={writeBlocked !== null || stagedCount === 0 || commitMsg.trim() === ''}
        title={writeBlocked ?? ''}
        onclick={() => { onCommit?.(commitMsg.trim()); commitMsg = ''; draft = null; }}
      >Commit {stagedCount} file{stagedCount === 1 ? '' : 's'}</button>
    </div>
  {/if}
</div>

{#snippet treeRow(node: TreeNode, depth: number)}
  {#if node.isDir}
    <button
      class="row dir"
      style="padding-left: {0.4 + depth * 0.85}rem"
      onclick={() => toggle(node.path)}
    >
      <span class="caret">{expanded[node.path] ? '▾' : '▸'}</span>
      <span class="ficon"><Icon name={folderIcon(expanded[node.path])} size={14} /></span>
      <span class="name">{node.name}</span>
      {#if changedDirs.has(node.path)}
        <span class="dir-dot" data-testid="tree-dir-changed" title="Has changed files" aria-label="has changed files"></span>
      {/if}
    </button>
    {#if expanded[node.path]}
      {#each node.children as child (child.path)}
        {@render treeRow(child, depth + 1)}
      {/each}
    {/if}
  {:else}
    <button
      class="row file"
      class:sel={selectedPath === node.path}
      style="padding-left: {0.4 + depth * 0.85 + 1.35}rem"
      onclick={() => onSelect(node.path, statusByPath.get(node.path))}
      title={node.path}
    >
      <span class="ficon"><Icon name={fileIcon(node.name)} size={14} /></span>
      <span class="name">{node.name}</span>
      {#if statusByPath.has(node.path)}
        {@const st = statusByPath.get(node.path) ?? ''}
        <span class="badge tb {BADGE[st]?.cls ?? 'b-mod'}" data-testid="tree-badge" title={st}>{BADGE[st]?.letter ?? '•'}</span>
      {/if}
    </button>
  {/if}
{/snippet}

<style>
  .list {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 0;
    height: 100%;
    border-right: 1px solid var(--border);
    min-width: 0;
  }
  .filter {
    margin: 0.35rem 0.5rem 0.35rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: var(--text-2xs);
    padding: 0.25rem 0.45rem;
    flex: 0 0 auto;
  }
  .rows {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
  }
  .hint {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
    padding: 0.5rem 0.7rem;
    margin: 0;
  }
  .hint.err {
    color: var(--danger);
  }
  .group {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0;
    padding: 0.4rem 0.5rem 0.2rem;
    font-size: var(--text-2xs);
    font-weight: 600;
    color: var(--fg-muted);
  }
  .gtoggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex: 1 1 auto;
    min-width: 0;
    background: transparent;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .gtoggle:hover {
    color: var(--fg);
  }
  .gcount {
    margin-left: auto;
    font-weight: 400;
  }
  .row.ranged {
    padding-left: 1.35rem;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    background: transparent;
    border: none;
    color: var(--fg);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.34rem 0.4rem;
    text-align: left;
  }
  .row:hover {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .row.sel {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .row .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.file .name {
    font-family: var(--mono);
  }
  .caret {
    flex: 0 0 auto;
    width: 0.95rem;
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
  .ficon {
    flex: 0 0 auto;
    display: inline-flex;
    justify-content: center;
    width: 1.2rem;
    color: var(--fg-muted);
  }
  .badge {
    flex: 0 0 auto;
    width: 1.1rem;
    height: 1.1rem;
    line-height: 1.1rem;
    text-align: center;
    border-radius: var(--radius-xs);
    font-size: var(--text-2xs);
    font-weight: 700;
  }
  .tb {
    margin-left: auto;
  }
  .dir-dot {
    flex: 0 0 auto;
    margin-left: auto;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--status-waiting);
  }
  .b-mod {
    background: var(--waiting-soft);
    color: var(--status-waiting);
  }
  .b-add {
    background: var(--done-soft);
    color: var(--status-done);
  }
  .b-del {
    background: var(--failed-soft);
    color: var(--status-failed);
  }
  .b-ren {
    background: var(--accent-soft);
    color: var(--status-working);
  }
  .b-unt {
    background: color-mix(in srgb, var(--fg-muted) 26%, transparent);
    color: var(--fg-muted);
  }
  .b-cnf {
    background: var(--failed-soft);
    color: var(--status-failed);
  }
  .row-wrap {
    display: flex;
    align-items: center;
  }
  .row-wrap .row {
    flex: 1 1 auto;
    min-width: 0;
  }
  .stage {
    flex: 0 0 auto;
    margin: 0 0.1rem 0 0.4rem;
    cursor: pointer;
    accent-color: var(--accent);
  }
  .commit-footer {
    border-top: 1px solid var(--border);
    padding: 0.4rem 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    flex: 0 0 auto;
  }
  .commit-footer .draft {
    align-self: flex-start;
    font-size: var(--text-2xs);
  }
  .commit-footer .draft-error {
    font-size: var(--text-2xs);
    color: var(--danger);
  }
  .commit-footer textarea {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: var(--text-2xs);
    font-family: inherit;
    resize: vertical;
    padding: 0.25rem 0.4rem;
    width: 100%;
    box-sizing: border-box;
  }
  .commit-footer button {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-pane));
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.25rem 0.5rem;
    text-align: center;
  }
  .commit-footer button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 30%, var(--bg-pane));
  }
  .commit-footer button:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
