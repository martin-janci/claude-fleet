<script lang="ts">
  import type { Branch } from './history';
  import Skeleton from './states/Skeleton.svelte';

  let {
    branches,
    loading,
    error,
    onCheckout,
    onDelete,
    onNew,
    onDeleteMerged = () => {},
    writeBlocked = null,
  }: {
    branches: Branch[];
    loading: boolean;
    error: string | null;
    onCheckout: (name: string) => void;
    onDelete: (name: string) => void;
    onNew: () => void;
    /** The local branches flagged merged, for one confirm-and-delete. */
    onDeleteMerged?: (names: string[]) => void;
    writeBlocked?: string | null;
  } = $props();

  // "Merged" narrows both groups to branches the base already contains; the
  // current branch and the base are never flagged, so they drop out too.
  let mergedOnly = $state(false);
  const shown = $derived(mergedOnly ? branches.filter((b) => b.merged) : branches);
  const locals = $derived(shown.filter((b) => !b.isRemote));
  const remotes = $derived(shown.filter((b) => b.isRemote));
  const mergedLocals = $derived(branches.filter((b) => !b.isRemote && b.merged).map((b) => b.name));
  const mergedRemotes = $derived(branches.filter((b) => b.isRemote && b.merged).length);
  const mergedCount = $derived(branches.filter((b) => b.merged).length);
</script>

<div class="branches" data-testid="branch-list">
  <div class="bbar">
    <button class="new" disabled={writeBlocked !== null} title={writeBlocked ?? ''} onclick={onNew}
      >+ New branch</button
    >
    <button
      class="chip"
      class:on={mergedOnly}
      aria-pressed={mergedOnly}
      data-testid="filter-merged"
      title="Show only branches the base branch already contains"
      onclick={() => (mergedOnly = !mergedOnly)}>Merged {mergedCount}</button
    >
    {#if mergedLocals.length}
      <button
        class="new del-merged"
        data-testid="delete-merged"
        disabled={writeBlocked !== null}
        title={writeBlocked ?? 'Delete the local branches the base branch already contains'}
        onclick={() => onDeleteMerged(mergedLocals)}>Delete merged ({mergedLocals.length})</button
      >
    {/if}
  </div>
  {#if loading}
    <Skeleton />
  {:else if error}
    <p class="hint err">{error}</p>
  {:else}
    <div class="group-label">Local</div>
    {#each locals as b (b.name)}
      <div class="brow" class:cur={b.isCurrent} data-testid="branch-row">
        <span class="bname">{b.isCurrent ? '● ' : ''}{b.name}</span>
        {#if b.merged}
          <span class="merged" data-testid="branch-merged">merged</span>
        {/if}
        {#if b.ahead || b.behind}
          <span class="track">{b.ahead ? `↑${b.ahead}` : ''}{b.behind ? `↓${b.behind}` : ''}</span>
        {/if}
        <span class="bactions">
          {#if !b.isCurrent}
            <button disabled={writeBlocked !== null} title={writeBlocked ?? ''} onclick={() => onCheckout(b.name)}>Checkout</button>
            <button class="del" disabled={writeBlocked !== null} title={writeBlocked ?? ''} onclick={() => onDelete(b.name)}>Delete</button>
          {/if}
        </span>
      </div>
    {:else}
      {#if mergedOnly}<p class="hint">No merged local branches.</p>{/if}
    {/each}
    {#if remotes.length}
      <div class="group-label">
        Remote {remotes.length}{#if !mergedOnly && mergedRemotes}<span class="gnote"> · {mergedRemotes} merged</span>{/if}
      </div>
      {#each remotes as b (b.name)}
        <div class="brow" data-testid="branch-row">
          <span class="bname">{b.name}</span>
          {#if b.merged}
            <span class="merged" data-testid="branch-merged">merged</span>
          {/if}
          <span class="bactions">
            <button disabled={writeBlocked !== null} title={writeBlocked ?? ''} onclick={() => onCheckout(b.name)}>Checkout</button>
          </span>
        </div>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .branches { font-size: var(--text-2xs); overflow: auto; height: 100%; }
  .bbar { padding: 0.4rem 0.5rem; display: flex; gap: 0.4rem; align-items: center; }
  .new, .chip {
    background: transparent; border: 1px solid var(--border); border-radius: var(--radius-sm);
    color: var(--fg); cursor: pointer; font-size: var(--text-2xs); padding: 0.2rem 0.5rem;
  }
  .new:disabled { opacity: 0.5; cursor: not-allowed; }
  .chip { color: var(--fg-muted); border-radius: var(--radius-pill); }
  .chip.on {
    color: var(--fg); border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }
  .del-merged { margin-left: auto; }
  .del-merged:not(:disabled):hover { color: var(--danger); border-color: var(--danger); }
  .group-label {
    color: var(--fg-muted); font-size: var(--text-2xs); text-transform: uppercase;
    padding: 0.4rem 0.6rem 0.2rem;
  }
  .gnote { text-transform: none; }
  .brow {
    display: flex; align-items: center; gap: 0.5rem; padding: 0.25rem 0.6rem;
  }
  .brow:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .brow:hover .bactions { visibility: visible; }
  .bname { flex: 1 1 auto; font-family: var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cur .bname { color: var(--accent); }
  .merged {
    flex: 0 0 auto; color: var(--fg-muted); font-size: var(--text-2xs);
    border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 0.4rem;
  }
  .track { flex: 0 0 auto; color: var(--fg-muted); font-size: var(--text-2xs); }
  .bactions { flex: 0 0 auto; visibility: hidden; display: flex; gap: 0.3rem; }
  .bactions button {
    background: transparent; border: 1px solid var(--border); border-radius: var(--radius-xs);
    color: var(--fg-muted); cursor: pointer; font-size: var(--text-2xs); padding: 0 0.4rem;
  }
  .bactions button:hover { color: var(--fg); border-color: var(--accent); }
  .bactions button.del:hover { color: var(--status-failed); border-color: var(--status-failed); }
  .hint { color: var(--fg-muted); padding: 0.5rem 0.7rem; }
  .hint.err { color: var(--danger); }
</style>
