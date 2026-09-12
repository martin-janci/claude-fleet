<script lang="ts">
  // The Add-project dialog's "My GitHub" mode: the repositories `gh` can see
  // on one host, filtered client-side. Picking a row hands its
  // `owner/repo` back to the dialog, which switches to clone mode with it.
  import { listGithubRepos, type GithubRepo } from './projects';
  import PickerList from './PickerList.svelte';
  import type { PickerItem } from './PickerList.svelte';

  let { host, onpick }: { host: string; onpick: (nameWithOwner: string) => void } = $props();

  type ListState =
    | { status: 'loading' }
    | { status: 'ready'; repos: GithubRepo[] }
    | { status: 'error'; message: string };
  let list = $state<ListState>({ status: 'loading' });
  let filter = $state('');
  let activeKey = $state<string | null>(null);

  // A slow reply for a previous host must not land after a newer request
  // (same guard as NewSessionDialog's `scanSeq`).
  let seq = 0;
  $effect(() => {
    const h = host;
    const mine = ++seq;
    list = { status: 'loading' };
    void listGithubRepos(h).then((r) => {
      if (mine !== seq) return;
      // gh's own stderr (not installed, "run gh auth login"…) is shown
      // verbatim — never an empty list pretending there are no repos.
      list = r.ok ? { status: 'ready', repos: r.value ?? [] } : { status: 'error', message: r.error.message };
    });
    return () => {
      seq++;
    };
  });

  const items: PickerItem[] = $derived.by(() => {
    if (list.status !== 'ready') return [];
    const q = filter.trim().toLowerCase();
    return list.repos
      .filter(
        (r) =>
          !q ||
          r.name_with_owner.toLowerCase().includes(q) ||
          (r.description ?? '').toLowerCase().includes(q),
      )
      .map((r) => ({
        key: r.name_with_owner,
        label: r.name_with_owner,
        description: r.description ?? undefined,
        meta: r.is_private ? 'private' : undefined,
        testid: 'gh-repo-row',
      }));
  });

  // Keep the highlight on a visible row as the filter narrows the list.
  $effect(() => {
    if (!items.some((i) => i.key === activeKey)) activeKey = items[0]?.key ?? null;
  });

  function onFilterKeydown(e: KeyboardEvent) {
    const i = items.findIndex((it) => it.key === activeKey);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = items[Math.max(0, Math.min(items.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (next) activeKey = next.key;
    } else if (e.key === 'Enter') {
      // Enter picks the highlighted repo rather than submitting the dialog.
      e.preventDefault();
      e.stopPropagation();
      if (activeKey) onpick(activeKey);
    }
  }
</script>

{#if list.status === 'loading'}
  <p class="status" data-testid="gh-loading">Listing repositories on {host}…</p>
{:else if list.status === 'error'}
  <p class="err" data-testid="gh-error">{list.message}</p>
{:else}
  <input
    id="gh-filter"
    data-testid="gh-filter"
    bind:value={filter}
    onkeydown={onFilterKeydown}
    placeholder="Filter repositories"
    aria-label="Filter repositories"
  />
  <PickerList
    {items}
    {activeKey}
    onactivate={(k) => (activeKey = k)}
    {onpick}
    maxHeight="14rem"
    ariaLabel="GitHub repositories"
    emptyText={list.repos.length === 0 ? `gh lists no repositories on ${host}.` : 'Nothing matches.'}
    testid="gh-list"
  />
{/if}

<style>
  .status { font-size: 0.75rem; color: var(--fg-muted); margin: 0; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; white-space: pre-wrap; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    min-width: 0;
  }
</style>
