<script lang="ts">
  import { repoFetch, repoPull, repoPush } from './history';
  import type { Result } from './result';
  import type { SessionRow } from './sessions';
  import { pushError } from './toasts';

  let {
    session,
    ondone,
    writeBlocked = null,
  }: { session: SessionRow; ondone: () => void; writeBlocked?: string | null } = $props();
  let busy = $state<string | null>(null);

  const FAILED: Record<string, string> = { fetch: 'Fetch failed', pull: 'Pull failed', push: 'Push failed' };

  // Review w16-11: a failure used to be a red "!" with git's stderr in a
  // title the keyboard could not reach. It is a toast in words now, with
  // git's own text under Details.
  async function run(label: string, fn: () => Promise<Result<unknown>>): Promise<void> {
    busy = label;
    const r = await fn();
    busy = null;
    if (r.ok) ondone();
    else pushError(r.error, FAILED[label]);
  }
</script>

<div class="remote">
  <button
    disabled={busy !== null || writeBlocked !== null}
    title={writeBlocked ?? ''}
    onclick={() => run('fetch', () => repoFetch(session.id))}
  >
    {busy === 'fetch' ? '…' : 'Fetch'}
  </button>
  <button
    disabled={busy !== null || writeBlocked !== null}
    title={writeBlocked ?? ''}
    onclick={() => run('pull', () => repoPull(session.id))}
  >
    {busy === 'pull' ? '…' : 'Pull'}
  </button>
  <button
    disabled={busy !== null || writeBlocked !== null}
    title={writeBlocked ?? ''}
    onclick={() => run('push', () => repoPush(session.id, false))}
  >
    {busy === 'push' ? '…' : 'Push'}
  </button>
</div>

<style>
  .remote { display: flex; gap: 0.3rem; align-items: center; }
  .remote button {
    background: transparent; border: 1px solid var(--border); border-radius: var(--radius-sm);
    color: var(--fg-muted); cursor: pointer; font-size: var(--text-2xs); padding: 0.15rem 0.5rem;
  }
  .remote button:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .remote button:disabled { opacity: 0.5; cursor: default; }
</style>
