<script lang="ts">
  // The Settings page tree and search (design §6): "General" (the
  // hand-written panels) and every generated page, plus a search over every
  // setting's label, key, help and tags. `@modified` lists what is off its
  // default; `@tag:experimental` and the like filter by tag. A hit opens the
  // setting's page and tab and highlights it.
  import { childrenOf, searchSettings, type Descriptor, type Page } from './pages';

  let {
    pages,
    descs,
    values,
    selected,
    counts = {},
    onselect,
  }: {
    pages: Page[];
    descs: Map<string, Descriptor>;
    values: Record<string, string>;
    /** `general`, or a page id. */
    selected: string;
    /** A badge by a page's name: proposals waiting for review. */
    counts?: Record<string, number>;
    onselect: (view: string, focusKey?: string) => void;
  } = $props();

  let query = $state('');
  const hits = $derived(searchSettings(query, pages, descs, values));

  // General first, then the Settings overview and its pages in list order,
  // then any other top-level page (Usage).
  const entries = $derived.by(() => {
    const out: { id: string; title: string; depth: number }[] = [
      { id: 'general', title: 'General', depth: 0 },
    ];
    for (const top of childrenOf(pages, null)) {
      out.push({ id: top.id, title: top.id === 'settings' ? 'Overview' : top.title, depth: 0 });
      for (const child of childrenOf(pages, top.id)) out.push({ id: child.id, title: child.title, depth: 1 });
    }
    return out;
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && query) {
      e.stopPropagation();
      query = '';
    } else if (e.key === 'Enter' && hits[0]) {
      onselect(hits[0].page, hits[0].key);
    }
  }
</script>

<nav class="settings-nav" aria-label="Settings pages">
  <input
    class="search"
    type="search"
    placeholder="Search settings"
    aria-label="Search settings"
    data-testid="settings-search"
    bind:value={query}
    onkeydown={onKey} />
  {#if query.trim()}
    <ul class="hits" data-testid="settings-search-hits">
      {#each hits as h (h.key)}
        <li>
          <button type="button" data-testid={`settings-hit-${h.key}`} onclick={() => onselect(h.page, h.key)}>
            <span>{h.label}</span>
            <span class="where">{h.pageTitle}</span>
          </button>
        </li>
      {:else}
        <li class="none">No setting matches.</li>
      {/each}
    </ul>
  {:else}
    <ul class="tree">
      {#each entries as e (e.id)}
        <li>
          <button
            type="button"
            class:depth1={e.depth === 1}
            aria-current={selected === e.id ? 'page' : undefined}
            data-testid={`settings-nav-${e.id}`}
            onclick={() => onselect(e.id)}
            >{e.title}{#if counts[e.id]}<span class="count" data-testid={`settings-nav-count-${e.id}`}
                aria-label={`${counts[e.id]} waiting`}>{counts[e.id]}</span
              >{/if}</button
          >
        </li>
      {/each}
    </ul>
  {/if}
</nav>

<style>
  .settings-nav {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 0;
  }
  .search {
    width: 100%;
    font-size: var(--control-font);
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li button {
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.45rem;
    font: inherit;
    font-size: 0.82rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    justify-content: space-between;
    gap: 0.4rem;
  }
  li button:hover {
    background: var(--control-bg-hover);
  }
  li button[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--accent);
  }
  li button:focus-visible {
    outline: var(--ring-w) solid var(--ring);
  }
  .depth1 {
    padding-left: 1.1rem;
  }
  .where {
    color: var(--fg-muted);
    font-size: 0.72rem;
  }
  .none {
    font-size: 0.8rem;
    color: var(--fg-muted);
    padding: 0.3rem 0.45rem;
  }
  .count {
    margin-left: 0.4rem;
    padding: 0 0.35rem;
    border-radius: 999px;
    background: var(--accent);
    color: var(--bg);
    font-size: 0.7rem;
  }
</style>
