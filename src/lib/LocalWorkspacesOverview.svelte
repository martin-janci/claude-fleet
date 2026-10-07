<script lang="ts">
  // Every local workspace link on this machine (Phase 3): state, change
  // counts, who drives it, and why it is stale when it is. Clean up stale
  // disconnects the stale ones; the files stay on both sides. Creating,
  // attaching, merging and removing worktrees stay where fleet already does
  // them (New session, the merge intent, Safe remove).
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { sessions } from './sessions';
  import {
    localWorkspaces,
    badgeFor,
    staleReason,
    DRIVER_LABEL,
    OPEN_APPS,
    openLocalWorkspace,
    syncLocalWorkspaceNow,
    pauseLocalWorkspace,
    resumeLocalWorkspace,
    disconnectLocalWorkspace,
    type LocalWorkspace,
  } from './local_workspaces';

  let { onclose }: { onclose: () => void } = $props();

  let busy = $state(false);
  let confirmCleanup = $state(false);
  let confirmDisconnect = $state<LocalWorkspace | null>(null);

  const rows = $derived(
    $localWorkspaces.map((w) => ({ w, badge: badgeFor(w), stale: staleReason(w, $sessions) })),
  );
  const stale = $derived(rows.filter((r) => r.stale).map((r) => r.w));

  async function run(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Local workspaces" {onclose} width="820px" testid="lw-overview">
  {#if rows.length === 0}
    <p class="muted">
      No worktree is linked to a folder on this machine. Open a session and use Local workspace in
      its details.
    </p>
  {:else}
    <ul class="links">
      {#each rows as { w, badge, stale: why } (w.id)}
        <li data-testid="lw-overview-row">
          <div class="head">
            <span class="dot tone-{badge.tone}"></span>
            <strong>{w.repo}</strong>
            <span class="muted">· {w.worktree_key} on {w.host_alias}</span>
            <span class="label">{badge.label}</span>
            {#if (w.driver ?? 'shared') !== 'shared'}
              <span class="chip">{DRIVER_LABEL[w.driver ?? 'shared']}</span>
            {/if}
          </div>
          <code class="path" title={w.local_path}>{w.local_path}</code>
          <div class="counts muted small">
            {w.local_activity ?? 0} yours · {w.remote_activity ?? 0} agent’s
            {#if w.conflicts.length > 0}· {w.conflicts.length} conflict{w.conflicts.length === 1
                ? ''
                : 's'}{/if}
          </div>
          {#if why}
            <p class="stale" data-testid="lw-stale">Stale: {why}</p>
          {/if}
          <div class="row">
            {#each OPEN_APPS as o (o.app)}
              <button class="ghost small" disabled={busy} onclick={() => run(() => openLocalWorkspace(w.id, o.app))}
                >{o.label}</button
              >
            {/each}
            <span class="sep"></span>
            <button
              class="ghost small"
              disabled={busy || w.paused}
              onclick={() => run(() => syncLocalWorkspaceNow(w.id))}>Sync now</button
            >
            {#if w.paused}
              <button class="ghost small" disabled={busy} onclick={() => run(() => resumeLocalWorkspace(w.id))}
                >Resume</button
              >
            {:else}
              <button class="ghost small" disabled={busy} onclick={() => run(() => pauseLocalWorkspace(w.id))}
                >Pause</button
              >
            {/if}
            <button class="ghost small" disabled={busy} onclick={() => (confirmDisconnect = w)}
              >Disconnect</button
            >
          </div>
        </li>
      {/each}
    </ul>
    <div class="foot">
      <p class="muted small">
        New and existing worktrees: New session. Merging a branch: Ask AI, “Get the branch ready to
        merge”. Removing a worktree: Safe remove on its session.
      </p>
      <button
        class="ghost"
        data-testid="lw-cleanup"
        disabled={busy || stale.length === 0}
        onclick={() => (confirmCleanup = true)}
      >Clean up stale ({stale.length})</button>
    </div>
  {/if}
</Modal>

{#if confirmCleanup}
  <ConfirmDialog
    title="Disconnect {stale.length} stale link{stale.length === 1 ? '' : 's'}?"
    message="Sync stops for them. The files stay where they are, on this machine and on the hosts."
    confirmLabel="Disconnect"
    confirmTestId="lw-cleanup-confirm"
    onconfirm={() => {
      confirmCleanup = false;
      const ids = stale.map((w) => w.id);
      void run(async () => {
        for (const id of ids) await disconnectLocalWorkspace(id);
      });
    }}
    oncancel={() => (confirmCleanup = false)}
  />
{/if}

{#if confirmDisconnect}
  {@const w = confirmDisconnect}
  <ConfirmDialog
    title="Disconnect the local workspace?"
    message={`Sync stops. The files stay where they are, in ${w.local_path} and on ${w.host_alias}.`}
    confirmLabel="Disconnect"
    onconfirm={() => {
      confirmDisconnect = null;
      void run(() => disconnectLocalWorkspace(w.id));
    }}
    oncancel={() => (confirmDisconnect = null)}
  />
{/if}

<style>
  .muted { margin: 0; color: var(--fg-muted); font-size: 0.85rem; }
  .small { font-size: 0.78rem; }
  .links { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.6rem; max-height: 60vh; overflow: auto; }
  .links li { border: 1px solid var(--border); border-radius: 6px; padding: 0.5rem 0.6rem; display: flex; flex-direction: column; gap: 0.25rem; }
  .head { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; font-size: 0.9rem; }
  .label { margin-left: auto; font-size: 0.82rem; }
  .chip { font-size: 0.7rem; border: 1px solid var(--accent); color: var(--accent); border-radius: 3px; padding: 0 0.35rem; }
  .path { font-family: var(--mono); font-size: 0.78rem; overflow-wrap: anywhere; }
  .stale { margin: 0; color: var(--usage-warn); font-size: 0.8rem; }
  .row { display: flex; gap: 0.3rem; flex-wrap: wrap; align-items: center; }
  .sep { width: 0.5rem; }
  .foot { display: flex; align-items: center; gap: 0.6rem; margin-top: 0.6rem; }
  .foot p { flex: 1; }
  .dot { width: 9px; height: 9px; border-radius: 50%; display: inline-block; flex: none; }
  .tone-ok { background: var(--usage-ok); }
  .tone-pending { background: var(--usage-warn); }
  .tone-conflict, .tone-error { background: var(--usage-crit); }
  .tone-idle { background: transparent; border: 1.5px solid var(--fg-muted); }
  .tone-off { background: var(--border); }
  .ghost {
    font-size: 0.85rem;
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
  }
  .ghost.small { font-size: 0.75rem; padding: 0.12rem 0.45rem; }
  .ghost:hover:not(:disabled) { border-color: var(--accent); }
  button:disabled { opacity: 0.55; cursor: default; }
</style>
