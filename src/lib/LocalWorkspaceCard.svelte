<script lang="ts">
  import Icon from './kit/Icon.svelte';
  // Local workspace sync (Phase 1): the session's worktree, kept in step with
  // a folder on this machine. Off: a folder field and Enable. On: state,
  // both paths, when it last synced, open conflicts with Keep local / Keep
  // remote, and Sync now / Pause / Resume / Disconnect. A desktop paired with
  // a hub syncs too: the folder and the SSH are this machine's.
  // Phases 2 and 3 add Open in…, what each side changed with Review changes,
  // who drives the worktree (Take over / Hand back to AI), Compare / Keep
  // both / Ask AI to resolve on a conflict, and the overview of every link.
  import { untrack } from 'svelte';
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
    OPEN_APPS,
    DRIVER_LABEL,
    openLocalWorkspace,
    setLocalWorkspaceDriver,
    dismissLocalActivity,
    compareLocalConflict,
    keepBothLocalConflict,
    askAiAboutLocalChanges,
    type FileDiff,
  } from './local_workspaces';
  import { projectById } from './projects';
  import { timeAgo } from './session_status';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import Modal from './Modal.svelte';
  import DiffView from './DiffView.svelte';
  import LocalChangesDialog from './LocalChangesDialog.svelte';
  import LocalWorkspacesOverview from './LocalWorkspacesOverview.svelte';
  import Loader from './Loader.svelte';

  let { session }: { session: SessionRow } = $props();

  const project = $derived(
    session.project_id != null ? $projectById.get(session.project_id)?.project : undefined,
  );
  const link = $derived(linkFor($localWorkspaces, session, project));
  const badge = $derived(badgeFor(link));

  let folder = $state('');
  let excludesText = $state('');
  let editingExcludes = $state(false);
  let busy = $state(false);
  let confirmDisconnect = $state(false);
  let reviewing = $state(false);
  let overview = $state(false);
  let comparing = $state<FileDiff | null>(null);

  const driver = $derived(link?.driver ?? 'shared');
  const localN = $derived(link?.local_activity ?? 0);
  const remoteN = $derived(link?.remote_activity ?? 0);

  // "Synced 3 s ago" keeps moving without a row event.
  // Only while there is a link to say it about (review r16).
  let nowMs = $state(Date.now());
  $effect(() => {
    if (!link) return;
    nowMs = Date.now();
    const clock = setInterval(() => (nowMs = Date.now()), 1000);
    return () => clearInterval(clock);
  });

  // The card outlives a switch to another session: what was typed for one
  // session must never be enabled for the next (it would bind this
  // worktree to the other repo's folder). The suggestion fills the field
  // once per session, so clearing it stays cleared.
  let shownFor: number | null = null;
  let seededFor: number | null = null;
  $effect(() => {
    const id = session.id;
    if (shownFor !== id) {
      shownFor = id;
      folder = '';
      excludesText = '';
      editingExcludes = false;
      confirmDisconnect = false;
      reviewing = false;
      comparing = null;
    }
    if (seededFor !== id && project) {
      seededFor = id;
      if (!untrack(() => folder)) folder = suggestedFolder(project.repo, session.worktree_key);
    }
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

  // Combining the two sides of a conflict (keep both, or the agent merges
  // them) is merging work: the kit's Liquid orbit runs while it does
  // (redesign step 5.13, motion.md "Fork, rebase or merge").
  let combining = $state(false);
  async function combine(fn: () => Promise<unknown>) {
    if (busy) return;
    combining = true;
    try {
      await run(fn);
    } finally {
      combining = false;
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
  {#if session.project_id == null}
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
    <div class="excludes">
      <label class="muted small" for="lw-leave-out-{session.id}">
        Leave out (optional): one pattern per line, .gitignore syntax. .git, target/,
        node_modules/ and the like are always left out.
      </label>
      <textarea
        id="lw-leave-out-{session.id}"
        rows="2"
        bind:value={excludesText}
        placeholder="*.log&#10;fixtures/large/"
        data-testid="lw-leave-out"
      ></textarea>
    </div>
    <div class="row">
      <button
        class="primary"
        disabled={busy || !folder.trim()}
        data-testid="lw-enable"
        onclick={() =>
          run(() =>
            enableLocalWorkspace(session.id, folder.trim(), excludesFromText(excludesText)),
          )}
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
    <div class="row" data-testid="lw-open">
      <span class="muted small">Open in</span>
      {#each OPEN_APPS as o (o.app)}
        <button
          class="ghost small"
          disabled={busy}
          data-testid="lw-open-{o.app}"
          onclick={() => run(() => openLocalWorkspace(link.id, o.app))}
        >{o.label}</button>
      {/each}
    </div>
    <div class="driver" data-testid="lw-driver">
      <span class="chip driver-{driver}">{DRIVER_LABEL[driver] ?? driver}</span>
      {#if driver !== 'developer'}
        <button
          class="ghost small"
          disabled={busy}
          data-testid="lw-take-over"
          title="Tell the agent to leave the files alone while you edit them"
          onclick={() => run(() => setLocalWorkspaceDriver(link.id, 'developer'))}
        >Take over</button>
      {/if}
      {#if driver !== 'agent'}
        <button
          class="ghost small"
          disabled={busy}
          data-testid="lw-hand-back"
          title="Send the agent the files you changed and let it continue"
          onclick={() => run(() => setLocalWorkspaceDriver(link.id, 'agent'))}
        >Hand back to AI</button>
      {/if}
      {#if driver !== 'shared'}
        <button
          class="ghost small"
          disabled={busy}
          onclick={() => run(() => setLocalWorkspaceDriver(link.id, 'shared'))}
        >Shared</button>
      {/if}
    </div>
    {#if localN > 0 || remoteN > 0}
      <div class="activity" data-testid="lw-activity">
        <span>
          {#if localN > 0}{localN} local change{localN === 1 ? '' : 's'}{/if}{#if localN > 0 && remoteN > 0}
            ·
          {/if}{#if remoteN > 0}{remoteN} agent change{remoteN === 1 ? '' : 's'}{/if}
        </span>
        <button
          class="ghost small"
          disabled={busy}
          data-testid="lw-review"
          onclick={() => (reviewing = true)}
        >Review changes…</button>
        {#if localN > 0}
          <button
            class="ghost small"
            disabled={busy}
            data-testid="lw-ask-continue"
            onclick={() => run(() => askAiAboutLocalChanges(link.id, 'continue'))}
          >Ask AI to continue</button>
        {/if}
        <button
          class="ghost small"
          disabled={busy}
          title="Mark them as looked at; the files stay as they are"
          onclick={() => run(() => dismissLocalActivity(link.id))}
        >Dismiss</button>
      </div>
    {:else}
      <div class="row">
        <button class="ghost small" disabled={busy} data-testid="lw-review" onclick={() => (reviewing = true)}
          >Review changes…</button
        >
      </div>
    {/if}
    {#if link.last_error}
      <p class="error" data-testid="lw-error">{link.last_error}</p>
    {/if}
    {#if link.skipped > 0}
      <p class="muted small">
        {link.skipped} file{link.skipped === 1 ? '' : 's'} left out (files over 64 MB, or links the sync cannot carry).
      </p>
    {/if}
    {#if link.conflicts.length > 0}
      <div class="conflicts" data-testid="lw-conflicts">
        <p class="warn"><Icon name="warning" size={12} /> Sync conflict: nothing was overwritten on either side.</p>
        {#if combining}
          <p class="combining" data-testid="lw-combining">
            <Loader name="liquid-orbit" size={40} label="Combining both sides" testid="lw-liquid" />
            <span class="muted small">Combining both sides…</span>
          </p>
        {/if}
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
                    data-testid="lw-compare"
                    onclick={() =>
                      run(async () => {
                        comparing = await compareLocalConflict(link.id, c.path);
                      })}
                  >Compare</button>
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
                  <button
                    class="ghost small"
                    disabled={busy}
                    data-testid="lw-keep-both"
                    title="Keep the remote file and save yours next to it as .local-copy"
                    onclick={() => combine(() => keepBothLocalConflict(link.id, c.path))}
                  >Keep both</button>
                  <button
                    class="ghost small"
                    disabled={busy}
                    data-testid="lw-ask-resolve"
                    title="Put your version next to the remote one and ask the agent to merge them"
                    onclick={() =>
                      combine(() => askAiAboutLocalChanges(link.id, 'resolve', { paths: [c.path] }))}
                  >Ask AI to resolve</button>
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
      >Disconnect…</button>
      <button class="ghost" data-testid="lw-overview-open" onclick={() => (overview = true)}
        >All local workspaces…</button
      >
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

{#if reviewing && link}
  <LocalChangesDialog {link} onclose={() => (reviewing = false)} />
{/if}

{#if comparing}
  <Modal title="Local → remote: {comparing.path}" onclose={() => (comparing = null)} width="760px" testid="lw-compare-dialog">
    {#if comparing.binary}
      <p class="muted">Binary file: the two versions differ.</p>
    {:else if !comparing.diff.trim()}
      <p class="muted">The two versions are the same.</p>
    {:else}
      <DiffView diff={comparing.diff} testid="lw-compare-diff" />
    {/if}
  </Modal>
{/if}

{#if overview}
  <LocalWorkspacesOverview onclose={() => (overview = false)} />
{/if}

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
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .hint { margin: 0; font-size: var(--text-xs); color: var(--fg-muted); line-height: 1.4; }
  .muted { margin: 0; color: var(--fg-muted); font-size: var(--text-xs); }
  .small { font-size: var(--text-2xs); }
  .row { display: flex; gap: 0.4rem; flex-wrap: wrap; align-items: center; }
  .path {
    flex: 1;
    min-width: 12rem;
    font-family: var(--mono);
    font-size: var(--text-xs);
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .ghost, .primary {
    font-size: var(--text-xs);
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost.small { font-size: var(--text-2xs); padding: 0.15rem 0.5rem; }
  .ghost:hover:not(:disabled) { border-color: var(--accent); }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }
  button:disabled { opacity: 0.55; cursor: default; }
  .status { display: flex; align-items: center; gap: 0.4rem; font-size: var(--text-sm); }
  .ago { color: var(--fg-muted); font-size: var(--text-2xs); }
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
    font-size: var(--text-2xs);
  }
  .paths dt { color: var(--fg-muted); }
  .paths dd { margin: 0; min-width: 0; }
  .paths code, .cpath {
    font-family: var(--mono);
    overflow-wrap: anywhere;
  }
  .error { margin: 0; color: var(--usage-crit); font-size: var(--text-2xs); }
  .warn { margin: 0; color: var(--usage-warn); font-size: var(--text-xs); }
  .combining { display: flex; align-items: center; gap: var(--space-2, 8px); margin: 0.3rem 0; }
  .conflicts ul { list-style: none; margin: 0.3rem 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .conflicts li { display: flex; flex-wrap: wrap; gap: 0.4rem; align-items: center; font-size: var(--text-2xs); }
  .kind { color: var(--fg-muted); }
  .cbtns { display: inline-flex; gap: 0.3rem; margin-left: auto; }
  .driver, .activity { display: flex; gap: 0.4rem; flex-wrap: wrap; align-items: center; font-size: var(--text-xs); }
  .activity span { color: var(--usage-warn); }
  .chip { font-size: var(--text-2xs); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 0.05rem 0.4rem; }
  .driver-developer { border-color: var(--usage-warn); color: var(--usage-warn); }
  .driver-agent { border-color: var(--accent); color: var(--accent); }
  .excludes textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--mono);
    font-size: var(--text-2xs);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
    padding: 0.35rem;
  }
</style>
