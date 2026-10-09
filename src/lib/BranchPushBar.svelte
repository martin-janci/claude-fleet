<!-- Push under the Changed list (redesign step 5.6, Files board): "Push 2 ↑"
     with where it goes and how far ahead the branch is. A branch no remote
     has yet is pushed with its upstream set, so the first push needs no
     terminal. Classic keeps Push in the History and Branches toolbars. -->
<script lang="ts">
  import { repoPush } from './history';
  import type { BranchDiff } from './files';
  import type { SessionRow } from './sessions';

  let {
    session,
    branch,
    ondone,
    writeBlocked = null,
  }: {
    session: SessionRow;
    branch: BranchDiff;
    ondone: () => void;
    writeBlocked?: string | null;
  } = $props();

  let busy = $state(false);
  let err = $state<string | null>(null);

  const ahead = $derived(branch.unpushed.length);
  const count = $derived(`${ahead}${branch.truncated ? '+' : ''}`);
  const where = $derived(
    branch.upstream ?? (branch.branch ? `${branch.branch}, not on the remote yet` : 'Detached HEAD'),
  );
  const why = $derived(
    writeBlocked ??
      (branch.branch === null
        ? 'Detached HEAD: check out a branch to push'
        : ahead === 0
          ? 'Nothing to push'
          : branch.upstream
            ? `Push ${count} commit${ahead === 1 ? '' : 's'} to ${branch.upstream}`
            : `Push ${branch.branch} and track it on the remote`),
  );
  const blocked = $derived(writeBlocked !== null || branch.branch === null || ahead === 0);

  async function push(): Promise<void> {
    busy = true;
    err = null;
    const r = await repoPush(session.id, branch.upstream === null);
    busy = false;
    if (r.ok) ondone();
    else err = r.error.message;
  }
</script>

<div class="pushbar" data-testid="branch-push-bar">
  <button
    type="button"
    class="push"
    data-testid="branch-push"
    disabled={busy || blocked}
    title={why}
    onclick={() => void push()}>{busy ? 'Pushing…' : `Push ${count} ↑`}</button
  >
  <span class="where" title={where}>{where}{#if ahead > 0}&nbsp;· {count} ahead{/if}</span>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</div>

<style>
  .pushbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.3rem 0.5rem;
    padding: 0.35rem 0.5rem;
    border-top: 1px solid var(--border);
    flex: 0 0 auto;
  }
  .push {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.2rem 0.6rem;
  }
  .push:hover:not(:disabled) {
    border-color: var(--accent);
  }
  .push:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .where {
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .err {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
