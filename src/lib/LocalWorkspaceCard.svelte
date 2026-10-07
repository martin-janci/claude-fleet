<script lang="ts">
  // Local workspace sync (Phase 1): the session's worktree, kept in step with
  // a folder on this machine. Off: a folder field and Enable. On: state,
  // both paths, when it last synced, open conflicts with Keep local / Keep
  // remote, and Sync now / Pause / Resume / Disconnect. A paired desktop shows
  // a note instead: the sync runs over this machine's own SSH.
  import { onDestroy } from 'svelte';
  import type { SessionRow } from './sessions';
  import {
    localWorkspaces,
    linkFor,
    badgeFor,
    CONFLICT_KIND_LABEL,
    enableLocalWorkspace,
    pauseLocalWorkspace,
    resumeLocalWorkspace,
    syncLocalWorkspaceNow,
    disconnectLocalWorkspace,
    resolveLocalConflict,
    setLocalWorkspaceExcludes,
    suggestedFolder,
  } from './local_workspaces';
  import { projectById } from './projects';
  import { hubStatus, ownsTheFleet } from './hub';
  import { timeAgo } from './session_status';
  import ConfirmDialog from './ConfirmDialog.svelte';

  let { session }: { session: SessionRow } = $props();

  const link = $derived(linkFor($localWorkspaces, session));
  const badge = $derived(badgeFor(link));
  const owns = $derived(ownsTheFleet($hubStatus));
  const project = $derived(
    session.project_id != null ? $projectById.get(session.project_id)?.project : undefined,
  );

  let folder = $state('');
  let excludesText = $state('');
  let editingExcludes = $state(false);
  let busy = $state(false);
  let confirmDisconnect = $state(false);

  // "Synced 3 s ago" keeps moving without a row event.
  let nowMs = $state(Date.now());
  const clock = setInterval(() => (nowMs = Date.now()), 1000);
  onDestroy(() => clearInterval(clock));

  $effect(() => {
    if (!folder && project) folder = suggestedFolder(project.repo, session.worktree_key);
  });

  async function pickFolder() {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === 'string') folder = picked;
    } catch {
      // No dialog (tests, a browser build): the text field still works.
    }
  }

  async function run(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } finally {
      busy = false;
    }
  }

  function excludesFromText(t: string): string[] {
    return t
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l.length > 0);
  }
</script>

<section class="block lw" data-testid="local-workspace">
  <h3>Local workspace</h3>
  {#if !owns}
    <p class="muted" data-testid="lw-remote-note">
      Local sync runs over this machine’s own SSH connection, so it is not available while this
      desktop is paired with a hub.
    </p>
  {:else if session.project_id == null}
    <p class="muted">This session is not in a project, so it has no worktree to sync.</p>
  {:else if !link}
    <p class="hint">
      Keep this session’s worktree in step with a folder on this machine, so you can open it in
      your IDE while the agent keeps working. Changes sync both ways; nothing is committed.
    </p>
    <div class="row">
      <input
        class="path"
        type="text"
        bind:value={folder}
        placeholder="/Users/you/fleet/repo"
        data-testid="lw-folder"
      />
      <button class="ghost" onclick={pickFolder} disabled={busy}>Choose…</button>
    </div>
    <div class="row">
      <button
        class="primary"
        disabled={busy || !folder.trim()}
        data-testid="lw-enable"
        onclick={() => run(() => enableLocalWorkspace(session.id, folder.trim()))}
      >
        Enable sync
      </button>
    </div>
  {:else}
    <div class="status" data-testid="lw-status">
      <span class="dot tone-{badge.tone}"></span>
      <span class="label">{badge.label}</span>
      {#if link.last_sync_at}
        <span class="ago">· checked {timeAgo(link.last_sync_at, nowMs)}</span>
      {/if}
    </div>
    <dl class="paths">
      <dt>Local</dt>
      <dd><code title={link.local_path}>{link.local_path}</code></dd>
      <dt>Remote</dt>
      <dd><code title={link.remote_path}>{link.host_alias}:{link.remote_path}</code></dd>
    </dl>
    {#if link.last_error}
      <p class="error" data-testid="lw-error">{link.last_error}</p>
    {/if}
    {#if link.skipped > 0}
      <p class="muted small">
        {link.skipped} file{link.skipped === 1 ? '' : 's'} left out (symlinks or files over 64 MB).
      </p>
    {/if}
    {#if link.conflicts.length > 0}
      <div class="conflicts" data-testid="lw-conflicts">
        <p class="warn">⚠ Sync conflict: nothing was overwritten on either side.</p>
        <ul>
          {#each link.conflicts as c (c.path)}
            <li data-testid="lw-conflict">
              <code class="cpath" title={c.path}>{c.path}</code>
              <span class="kind">{CONFLICT_KIND_LABEL[c.kind] ?? c.kind}</span>
              {#if c.resolution}
                <span class="muted small">keeping {c.resolution}…</span>
              {:else}
                <span class="cbtns">
                  <button
                    class="ghost small"
                    disabled={busy}
                    onclick={() => run(() => resolveLocalConflict(link.id, c.path, 'local'))}
                  >Keep local</button>
                  <button
                    class="ghost small"
                    disabled={busy}
                    onclick={() => run(() => resolveLocalConflict(link.id, c.path, 'remote'))}
                  >Keep remote</button>
                </span>
              {/if}
            </li>
          {/each}
        </ul>
        <p class="muted small">
          Or resolve it by hand: once both sides have the same content, the conflict clears.
        </p>
      </div>
    {/if}
    <div class="row">
      <button
        class="ghost"
        disabled={busy || link.paused}
        data-testid="lw-sync-now"
        onclick={() => run(() => syncLocalWorkspaceNow(link.id))}
      >Sync now</button>
      {#if link.paused}
        <button
          class="ghost"
          disabled={busy}
          data-testid="lw-resume"
          onclick={() => run(() => resumeLocalWorkspace(link.id))}
        >Resume</button>
      {:else}
        <button
          class="ghost"
          disabled={busy}
          data-testid="lw-pause"
          onclick={() => run(() => pauseLocalWorkspace(link.id))}
        >Pause</button>
      {/if}
      <button
        class="ghost"
        disabled={busy}
        onclick={() => {
          excludesText = link.excludes.join('\n');
          editingExcludes = !editingExcludes;
        }}
      >Excludes…</button>
      <button
        class="ghost"
        disabled={busy}
        data-testid="lw-disconnect"
        onclick={() => (confirmDisconnect = true)}
      >Disconnect</button>
    </div>
    {#if editingExcludes}
      <div class="excludes">
        <p class="muted small">
          One pattern per line, .gitignore syntax. Always left out: .git, target/, build/,
          .gradle/, node_modules/, .idea/, .vscode/, dist/, out/ and the like (<code>!build/</code>
          brings one back).
        </p>
        <textarea rows="4" bind:value={excludesText} data-testid="lw-excludes"></textarea>
        <div class="row">
          <button
            class="ghost"
            disabled={busy}
            onclick={() =>
              run(async () => {
                if (await setLocalWorkspaceExcludes(link.id, excludesFromText(excludesText))) {
                  editingExcludes = false;
                }
              })}
          >Save excludes</button>
        </div>
      </div>
    {/if}
  {/if}
</section>

{#if confirmDisconnect && link}
  <ConfirmDialog
    title="Disconnect the local workspace?"
    message={`Sync stops. The files stay where they are, in ${link.local_path} and on ${link.host_alias}.`}
    confirmLabel="Disconnect"
    confirmTestId="lw-disconnect-confirm"
    onconfirm={() => {
      confirmDisconnect = false;
      const id = link.id;
      void run(() => disconnectLocalWorkspace(id));
    }}
    oncancel={() => (confirmDisconnect = false)}
  />
{/if}

<style>
  .lw { display: flex; flex-direction: column; gap: 0.4rem; }
  .lw h3 {
    margin: 0 0 0.2rem 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .hint { margin: 0; font-size: 0.85rem; color: var(--fg-muted); line-height: 1.4; }
  .muted { margin: 0; color: var(--fg-muted); font-size: 0.85rem; }
  .small { font-size: 0.78rem; }
  .row { display: flex; gap: 0.4rem; flex-wrap: wrap; align-items: center; }
  .path {
    flex: 1;
    min-width: 12rem;
    font-family: var(--mono);
    font-size: 0.85rem;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg);
    color: var(--fg);
  }
  .ghost, .primary {
    font-size: 0.85rem;
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
  }
  .ghost.small { font-size: 0.78rem; padding: 0.15rem 0.5rem; }
  .ghost:hover:not(:disabled) { border-color: var(--accent); }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }
  button:disabled { opacity: 0.55; cursor: default; }
  .status { display: flex; align-items: center; gap: 0.4rem; font-size: 0.9rem; }
  .ago { color: var(--fg-muted); font-size: 0.8rem; }
  .dot { width: 9px; height: 9px; border-radius: 50%; display: inline-block; flex: none; }
  .tone-ok { background: var(--usage-ok); }
  .tone-pending { background: var(--usage-warn); }
  .tone-conflict, .tone-error { background: var(--usage-crit); }
  .tone-idle { background: transparent; border: 1.5px solid var(--fg-muted); }
  .tone-off { background: var(--border); }
  .paths {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.15rem 0.6rem;
    margin: 0;
    font-size: 0.8rem;
  }
  .paths dt { color: var(--fg-muted); }
  .paths dd { margin: 0; min-width: 0; }
  .paths code, .cpath {
    font-family: var(--mono);
    overflow-wrap: anywhere;
  }
  .error { margin: 0; color: var(--usage-crit); font-size: 0.82rem; }
  .warn { margin: 0; color: var(--usage-warn); font-size: 0.85rem; }
  .conflicts ul { list-style: none; margin: 0.3rem 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .conflicts li { display: flex; flex-wrap: wrap; gap: 0.4rem; align-items: center; font-size: 0.82rem; }
  .kind { color: var(--fg-muted); }
  .cbtns { display: inline-flex; gap: 0.3rem; margin-left: auto; }
  .excludes textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--mono);
    font-size: 0.8rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg);
    color: var(--fg);
    padding: 0.35rem;
  }
</style>
