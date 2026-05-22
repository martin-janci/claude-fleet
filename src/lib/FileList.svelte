<script lang="ts" module>
  import type { ChangedFile, RepoTree } from './files';

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
  } = $props();

  let filter = $state('');
  let commitMsg = $state('');
  // Folder expand state — keyed by dir path. Plain object so $state proxies it.
  let expanded = $state<Record<string, boolean>>({});

  const stagedCount = $derived(changes.filter((c) => c.staged).length);

  const statusByPath = $derived(new Map(changes.map((c) => [c.path, c.status])));

  const filterLc = $derived(filter.trim().toLowerCase());

  const filteredChanges = $derived(
    filterLc === ''
      ? changes
      : changes.filter((c) => c.path.toLowerCase().includes(filterLc)),
  );

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
      <p class="hint">Loading…</p>
    {:else if error}
      <p class="hint err">{error}</p>
    {:else if mode === 'changes'}
      {#if filteredChanges.length === 0}
        <p class="hint">{changes.length === 0 ? 'No changes.' : 'No matches.'}</p>
      {:else}
        {#each filteredChanges as c (c.path)}
          <div class="row-wrap">
            {#if enableStaging}
              <input
                type="checkbox"
                class="stage"
                checked={c.staged}
                title={c.staged ? 'Unstage' : 'Stage'}
                onclick={(e) => { e.stopPropagation(); onStageToggle?.(c.path, !c.staged); }}
              />
            {/if}
            <button
              class="row file"
              class:sel={selectedPath === c.path}
              onclick={() => onSelect(c.path, c.status)}
              title={c.orig_path ? `${c.orig_path} → ${c.path}` : c.path}
            >
              <span class="badge {BADGE[c.status]?.cls ?? 'b-mod'}"
                >{BADGE[c.status]?.letter ?? '•'}</span
              >
              <span class="name">{c.path}</span>
            </button>
          </div>
        {/each}
      {/if}
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
      <textarea bind:value={commitMsg} placeholder="Commit message…" rows={2}></textarea>
      <button
        disabled={stagedCount === 0 || commitMsg.trim() === ''}
        onclick={() => { onCommit?.(commitMsg.trim()); commitMsg = ''; }}
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
      <span class="name">{node.name}</span>
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
      style="padding-left: {0.4 + depth * 0.85 + 0.95}rem"
      onclick={() => onSelect(node.path, statusByPath.get(node.path))}
      title={node.path}
    >
      <span class="name">{node.name}</span>
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
    border-radius: 4px;
    color: var(--fg);
    font-size: 0.78rem;
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
    font-size: 0.82rem;
    padding: 0.5rem 0.7rem;
    margin: 0;
  }
  .hint.err {
    color: #e64a4a;
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
    font-size: 0.78rem;
    padding: 0.18rem 0.4rem;
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
    font-family: var(--mono, ui-monospace, monospace);
  }
  .caret {
    flex: 0 0 auto;
    width: 0.95rem;
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
  .badge {
    flex: 0 0 auto;
    width: 1.1rem;
    height: 1.1rem;
    line-height: 1.1rem;
    text-align: center;
    border-radius: 3px;
    font-size: 0.66rem;
    font-weight: 700;
  }
  .b-mod {
    background: color-mix(in srgb, #d9a000 30%, transparent);
    color: #d9a000;
  }
  .b-add {
    background: color-mix(in srgb, #3fb950 30%, transparent);
    color: #3fb950;
  }
  .b-del {
    background: color-mix(in srgb, #f85149 30%, transparent);
    color: #f85149;
  }
  .b-ren {
    background: color-mix(in srgb, #58a6ff 30%, transparent);
    color: #58a6ff;
  }
  .b-unt {
    background: color-mix(in srgb, var(--fg-muted) 26%, transparent);
    color: var(--fg-muted);
  }
  .b-cnf {
    background: color-mix(in srgb, #db6d28 32%, transparent);
    color: #db6d28;
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
  .commit-footer textarea {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--fg);
    font-size: 0.78rem;
    font-family: inherit;
    resize: vertical;
    padding: 0.25rem 0.4rem;
    width: 100%;
    box-sizing: border-box;
  }
  .commit-footer button {
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-pane));
    border: 1px solid var(--accent);
    border-radius: 4px;
    color: var(--fg);
    cursor: pointer;
    font-size: 0.78rem;
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
