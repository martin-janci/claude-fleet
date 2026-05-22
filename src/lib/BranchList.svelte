<script lang="ts">
  import type { Branch } from './history';

  let {
    branches,
    loading,
    error,
    onCheckout,
    onDelete,
    onNew,
  }: {
    branches: Branch[];
    loading: boolean;
    error: string | null;
    onCheckout: (name: string) => void;
    onDelete: (name: string) => void;
    onNew: () => void;
  } = $props();

  const locals = $derived(branches.filter((b) => !b.isRemote));
  const remotes = $derived(branches.filter((b) => b.isRemote));
</script>

<div class="branches" data-testid="branch-list">
  <div class="bbar">
    <button class="new" onclick={onNew}>+ New branch</button>
  </div>
  {#if loading}
    <p class="hint">Loading…</p>
  {:else if error}
    <p class="hint err">{error}</p>
  {:else}
    <div class="group-label">Local</div>
    {#each locals as b (b.name)}
      <div class="brow" class:cur={b.isCurrent}>
        <span class="bname">{b.isCurrent ? '● ' : ''}{b.name}</span>
        {#if b.ahead || b.behind}
          <span class="track">{b.ahead ? `↑${b.ahead}` : ''}{b.behind ? `↓${b.behind}` : ''}</span>
        {/if}
        <span class="bactions">
          {#if !b.isCurrent}
            <button onclick={() => onCheckout(b.name)}>Checkout</button>
            <button class="del" onclick={() => onDelete(b.name)}>Delete</button>
          {/if}
        </span>
      </div>
    {/each}
    {#if remotes.length}
      <div class="group-label">Remote</div>
      {#each remotes as b (b.name)}
        <div class="brow">
          <span class="bname">{b.name}</span>
          <span class="bactions">
            <button onclick={() => onCheckout(b.name)}>Checkout</button>
          </span>
        </div>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .branches { font-size: 0.8rem; overflow: auto; height: 100%; }
  .bbar { padding: 0.4rem 0.5rem; }
  .new {
    background: transparent; border: 1px solid var(--border); border-radius: 4px;
    color: var(--fg); cursor: pointer; font-size: 0.74rem; padding: 0.2rem 0.5rem;
  }
  .group-label {
    color: var(--fg-muted); font-size: 0.68rem; text-transform: uppercase;
    padding: 0.4rem 0.6rem 0.2rem;
  }
  .brow {
    display: flex; align-items: center; gap: 0.5rem; padding: 0.25rem 0.6rem;
  }
  .brow:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .brow:hover .bactions { visibility: visible; }
  .bname { flex: 1 1 auto; font-family: var(--mono, monospace); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cur .bname { color: var(--accent); }
  .track { flex: 0 0 auto; color: var(--fg-muted); font-size: 0.72rem; }
  .bactions { flex: 0 0 auto; visibility: hidden; display: flex; gap: 0.3rem; }
  .bactions button {
    background: transparent; border: 1px solid var(--border); border-radius: 3px;
    color: var(--fg-muted); cursor: pointer; font-size: 0.7rem; padding: 0 0.4rem;
  }
  .bactions button:hover { color: var(--fg); border-color: var(--accent); }
  .bactions button.del:hover { color: #f85149; border-color: #f85149; }
  .hint { color: var(--fg-muted); padding: 0.5rem 0.7rem; }
  .hint.err { color: #e64a4a; }
</style>
