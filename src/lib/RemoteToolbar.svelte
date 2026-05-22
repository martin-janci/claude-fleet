<script lang="ts">
  import { repoFetch, repoPull, repoPush } from './history';
  import type { Result } from './result';
  import type { SessionRow } from './sessions';

  let { session, ondone }: { session: SessionRow; ondone: () => void } = $props();
  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);

  async function run(label: string, fn: () => Promise<Result<unknown>>): Promise<void> {
    busy = label;
    err = null;
    const r = await fn();
    busy = null;
    if (r.ok) ondone();
    else err = r.error.message;
  }
</script>

<div class="remote">
  <button disabled={busy !== null} onclick={() => run('fetch', () => repoFetch(session.id))}>
    {busy === 'fetch' ? '…' : 'Fetch'}
  </button>
  <button disabled={busy !== null} onclick={() => run('pull', () => repoPull(session.id))}>
    {busy === 'pull' ? '…' : 'Pull'}
  </button>
  <button disabled={busy !== null} onclick={() => run('push', () => repoPush(session.id, false))}>
    {busy === 'push' ? '…' : 'Push'}
  </button>
  {#if err}<span class="err" title={err}>!</span>{/if}
</div>

<style>
  .remote { display: flex; gap: 0.3rem; align-items: center; }
  .remote button {
    background: transparent; border: 1px solid var(--border); border-radius: 4px;
    color: var(--fg-muted); cursor: pointer; font-size: 0.72rem; padding: 0.15rem 0.5rem;
  }
  .remote button:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .remote button:disabled { opacity: 0.5; cursor: default; }
  .err { color: #f85149; font-weight: 700; cursor: help; }
</style>
