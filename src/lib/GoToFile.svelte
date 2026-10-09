<!-- Go to file (redesign step 5.6, ⌥⌘P / Ctrl+Alt+P, Files tab only): type
     part of a path, ↑/↓ to move, Enter opens it in the tree view. -->
<script lang="ts">
  import Modal from './Modal.svelte';
  import { goToFileMatches } from './files';

  let {
    entries,
    loading = false,
    onpick,
    onclose,
  }: {
    /** The worktree's paths; empty while the tree loads. */
    entries: readonly string[];
    loading?: boolean;
    onpick: (path: string) => void;
    onclose: () => void;
  } = $props();

  let query = $state('');
  let active = $state(0);
  const matches = $derived(goToFileMatches(entries, query));
  $effect(() => {
    void query;
    active = 0;
  });

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      active = Math.min(active + 1, matches.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      active = Math.max(active - 1, 0);
    } else if (e.key === 'Enter' && !e.isComposing) {
      e.preventDefault();
      const p = matches[active];
      if (p) onpick(p);
    }
  }

  function focusOnMount(el: HTMLInputElement): void {
    el.focus();
  }
</script>

<Modal title="Go to file" {onclose} width="560px" testid="go-to-file">
  <input
    class="q"
    type="text"
    role="combobox"
    aria-expanded="true"
    aria-controls="go-to-file-list"
    aria-activedescendant={matches[active] ? `gtf-${active}` : undefined}
    aria-label="File path"
    placeholder="Type part of a path"
    spellcheck="false"
    autocomplete="off"
    data-testid="go-to-file-input"
    bind:value={query}
    {onkeydown}
    use:focusOnMount
  />
  <ul class="list" id="go-to-file-list" role="listbox" aria-label="Matching files">
    {#each matches as p, i (p)}
      <li
        id="gtf-{i}"
        role="option"
        aria-selected={i === active}
        class:active={i === active}
        data-testid="go-to-file-row"
        onmousemove={() => (active = i)}
        onclick={() => onpick(p)}
        onkeydown={() => {}}
      >
        <span class="name">{p.slice(p.lastIndexOf('/') + 1)}</span>
        <span class="dir">{p.slice(0, Math.max(0, p.lastIndexOf('/')))}</span>
      </li>
    {/each}
  </ul>
  {#if matches.length === 0}
    <p class="hint">{loading ? 'Reading the worktree…' : 'No file matches.'}</p>
  {/if}
</Modal>

<style>
  .q {
    width: 100%;
    box-sizing: border-box;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: var(--text-sm);
    padding: 0.4rem 0.55rem;
  }
  .list {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
    max-height: 50vh;
    overflow: auto;
  }
  li {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    min-height: 24px;
    padding: 0.2rem 0.5rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  li.active {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .name {
    font-family: var(--mono);
    font-size: var(--text-2xs);
    color: var(--fg);
    white-space: nowrap;
  }
  .dir {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .hint {
    margin: 0.5rem 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
</style>
