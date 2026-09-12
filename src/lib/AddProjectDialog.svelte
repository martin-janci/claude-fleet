<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { addProject, confirmTokenOf, type AddProjectSource, type ProjectTreeRow } from './projects';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';
  import { isComponent, parseRepoUrl } from './repo_url';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import GithubRepoBrowser from './GithubRepoBrowser.svelte';
  import {
    fleetSettings,
    loadFleetSettings,
    settingPathMap,
    settingLayout,
    projectDir,
    projectsDefaultRoot,
    PROJECTS_RESOLVED_KEY,
  } from './fleet_settings';

  let {
    onCreated,
    onCancel,
  }: {
    onCreated: (row: ProjectTreeRow) => void;
    onCancel: () => void;
  } = $props();

  // ── Modes ────────────────────────────────────────────────────────────
  type Mode = 'clone' | 'github' | 'folder' | 'new';
  const MODES: { id: Mode; label: string }[] = [
    { id: 'clone', label: 'Clone URL' },
    { id: 'github', label: 'My GitHub' },
    { id: 'folder', label: 'Existing folder' },
    { id: 'new', label: 'New project' },
  ];
  let mode = $state<Mode>('clone');

  // ── Host ─────────────────────────────────────────────────────────────
  // Same chips and rule as NewSessionDialog: visible, and reachable unless
  // `local`. Folder mode is local-only; it overrides the choice without
  // touching it, so leaving folder mode restores the remembered host.
  const isString = (v: unknown): v is string => typeof v === 'string';
  const pickable = (alias: string) =>
    $hosts.some((h) => h.alias === alias && !h.hidden && (h.reachable || h.alias === 'local'));
  let chosenHost = $state<string>(
    untrack(() => {
      const last = readPref('last-host', '', isString);
      return last && pickable(last) ? last : 'local';
    }),
  );
  $effect(() => {
    writePref('last-host', chosenHost);
  });
  const host = $derived(mode === 'folder' ? 'local' : chosenHost);

  // ── Fields ───────────────────────────────────────────────────────────
  let url = $state('');
  let folderPath = $state<string | null>(null);
  let owner = $state('');
  let repo = $state('');
  let createRemote = $state(false);

  const parsed = $derived(parseRepoUrl(url));
  const ownerErr = $derived(
    isComponent(owner, 39) ? null : "Owner: 1–39 of A–Z a–z 0–9 . _ -, not starting with '-'",
  );
  const repoErr = $derived(
    isComponent(repo, 100) ? null : "Repository: 1–100 of A–Z a–z 0–9 . _ -, not starting with '-'",
  );

  /** Why Create is disabled, or null. `shown` is false for a reason the
   *  empty form already makes obvious. */
  const blocked = $derived.by((): { reason: string; shown: boolean } | null => {
    switch (mode) {
      case 'clone':
        return parsed
          ? null
          : {
              reason: 'Not a GitHub repository — use owner/repo, https://github.com/owner/repo or git@github.com:owner/repo.git',
              shown: url.trim() !== '',
            };
      case 'github':
        return { reason: 'Pick a repository', shown: false };
      case 'folder':
        return folderPath ? null : { reason: 'Choose a folder', shown: false };
      case 'new': {
        const err = ownerErr ?? repoErr;
        return err ? { reason: err, shown: owner !== '' || repo !== '' } : null;
      }
    }
  });

  // ── Destination preview ──────────────────────────────────────────────
  onMount(() => {
    // The preview needs the backend's per-host roots; best effort.
    void loadFleetSettings();
  });
  const layout = $derived(settingLayout($fleetSettings));
  const hostRoot = $derived(
    settingPathMap($fleetSettings, PROJECTS_RESOLVED_KEY)[host] ?? projectsDefaultRoot(layout),
  );
  const pathPreview = $derived.by((): string | null => {
    if (mode === 'folder') return folderPath;
    const target = mode === 'clone' ? parsed : mode === 'new' && !ownerErr && !repoErr ? { owner, repo } : null;
    return target ? projectDir(hostRoot, layout, target.owner, target.repo) : null;
  });

  function source(): AddProjectSource | null {
    if (blocked) return null;
    if (mode === 'clone') return { kind: 'clone', url: url.trim() };
    if (mode === 'folder' && folderPath) return { kind: 'folder', path: folderPath };
    if (mode === 'new') return { kind: 'new', owner, repo, create_remote: createRemote };
    return null;
  }

  async function chooseFolder() {
    try {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === 'string') folderPath = picked;
    } catch (e) {
      error = `Couldn't open the folder picker: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  function pickGithubRepo(nameWithOwner: string) {
    url = nameWithOwner;
    mode = 'clone';
  }

  // ── Create ───────────────────────────────────────────────────────────
  let busy = $state(false);
  /** Host of the request in flight — decides "Cancel" vs "Stop waiting". */
  let busyHost = $state('local');
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;
  type NewSource = Extract<AddProjectSource, { kind: 'new' }>;
  /** A `create_remote` request the backend wants confirmed. Held only while
   *  the confirmation is on screen; the token is never stored anywhere. */
  let pendingConfirm = $state<{ host: string; source: NewSource; token: string } | null>(null);
  let destroyed = false;
  onMount(() => () => {
    destroyed = true;
    controller?.abort();
  });

  async function submit() {
    if (busy || pendingConfirm) return;
    const s = source();
    if (s) await run(host, s);
  }

  async function run(h: string, s: AddProjectSource) {
    busy = true;
    busyHost = h;
    error = null;
    controller = new AbortController();
    const r = await addProject(h, s, controller.signal);
    controller = null;
    busy = false;
    if (destroyed) return;
    if (r.ok) {
      onCreated(r.value);
      return;
    }
    const remote = s.kind === 'new' && s.create_remote;
    const confirmed = s.kind === 'new' && s.confirm !== undefined;
    if (r.error.code === 'E_CONFIRM_REQUIRED') {
      const token = confirmTokenOf(r.error);
      if (s.kind === 'new' && remote && token && !confirmed) {
        // Never retried automatically: only the confirmation's own button
        // resends, with exactly this token.
        pendingConfirm = { host: h, source: s, token };
      } else {
        error = confirmed
          ? 'The GitHub confirmation expired or was already used — press Create to confirm again.'
          : r.error.message;
      }
      return;
    }
    if (r.error.code === 'E_CANCELLED') {
      // A cancelled GitHub creation may have happened anyway: the backend's
      // message says so, and it must not be swallowed.
      error = remote ? r.error.message : h === 'local' ? 'Cancelled.' : `Stopped waiting — ${h} may still finish.`;
      return;
    }
    // Keep every field (an E_GH retry resumes the same owner/repo).
    error = r.error.message;
  }

  function confirmCreate() {
    const p = pendingConfirm;
    if (!p) return;
    pendingConfirm = null;
    void run(p.host, { ...p.source, confirm: p.token });
  }

  function cancelCreate() {
    controller?.abort();
  }

  // Enter in a field creates. Escape is handled by <Modal>.
  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.shiftKey || e.altKey) return;
    if ((e.target as HTMLElement | null)?.tagName === 'INPUT') {
      e.preventDefault();
      void submit();
    }
  }
</script>

<!-- Escape while a request is in flight does nothing: closing would drop
     the outcome (and a GitHub hedge) on the floor. Use Cancel/Stop waiting. -->
<Modal label="Add project" onclose={() => { if (!busy) onCancel(); }} width="460px" testid="add-project-dialog">
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog" onkeydown={onKeydown}>
  <h3>Add project</h3>

  <div class="fields">
    <div class="seg" role="group" aria-label="Source">
      {#each MODES as m (m.id)}
        <button
          type="button"
          class="seg-pick"
          class:active={mode === m.id}
          data-testid="add-mode-{m.id}"
          onclick={() => (mode = m.id)}
        >{m.label}</button>
      {/each}
    </div>

    <label for="add-host-picker">Host</label>
    <div class="host-row" id="add-host-picker" role="group">
      {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
        {@const folderOnly = mode === 'folder' && h.alias !== 'local'}
        <button
          type="button"
          class="host-pick"
          class:active={host === h.alias}
          disabled={(!h.reachable && h.alias !== 'local') || folderOnly}
          title={folderOnly ? 'An existing folder can only be adopted on local — it is picked on this machine' : undefined}
          onclick={() => (chosenHost = h.alias)}
        >{h.alias}</button>
      {/each}
    </div>

    {#if mode === 'clone'}
      <label for="add-url">Repository</label>
      <input
        id="add-url"
        data-testid="clone-url"
        data-autofocus
        bind:value={url}
        placeholder="owner/repo or https://github.com/owner/repo"
      />
    {:else if mode === 'github'}
      <label for="gh-filter">Repositories on {host}</label>
      <GithubRepoBrowser {host} onpick={pickGithubRepo} />
    {:else if mode === 'folder'}
      <label for="choose-folder">Folder (an existing git checkout)</label>
      <button type="button" id="choose-folder" class="pick-folder" data-testid="choose-folder" onclick={chooseFolder}>
        Choose folder…
      </button>
    {:else}
      <label for="new-owner">Owner</label>
      <input id="new-owner" data-testid="new-owner" data-autofocus bind:value={owner} maxlength="39" placeholder="martin-janci" />
      <label for="new-repo">Repository name</label>
      <input id="new-repo" data-testid="new-repo" bind:value={repo} maxlength="100" placeholder="my-project" />
      <label class="check">
        <input type="checkbox" data-testid="new-create-remote" bind:checked={createRemote} />
        Also create a private repository on GitHub
      </label>
    {/if}

    {#if pathPreview}
      <p class="preview" data-testid="add-path-preview" title={pathPreview}>
        <span class="k">{mode === 'folder' ? 'folder' : 'into'}</span> <code>{pathPreview}</code>
      </p>
    {/if}
    {#if blocked?.shown}
      <p class="err" data-testid="add-reason">{blocked.reason}</p>
    {/if}
    {#if error}
      <p class="err" data-testid="add-error">{error}</p>
    {/if}
  </div>

  <div class="actions">
    {#if busy}
      <button
        type="button"
        data-testid="cancel-create"
        onclick={cancelCreate}
        title={busyHost === 'local'
          ? 'Stops the operation on this machine; a GitHub repository may already have been created'
          : `${busyHost} may still finish the clone or the GitHub creation after you stop waiting`}
      >{busyHost === 'local' ? 'Cancel' : 'Stop waiting'}</button>
    {:else}
      <button type="button" onclick={onCancel}>Cancel</button>
      <button type="button" class="primary" data-testid="add-create" onclick={submit} disabled={!!blocked}>Create</button>
    {/if}
  </div>
</div>
</Modal>

{#if pendingConfirm}
  <ConfirmDialog
    title="Create on GitHub?"
    confirmLabel="Create on GitHub"
    onconfirm={confirmCreate}
    oncancel={() => (pendingConfirm = null)}
    confirmTestId="confirm-create-remote"
  >
    This creates a <strong>private</strong> repository
    <code>{pendingConfirm.source.owner}/{pendingConfirm.source.repo}</code> on GitHub (via
    <code>{pendingConfirm.host}</code>) and pushes the initial commit to it.
  </ConfirmDialog>
{/if}

<style>
  .dialog { display: flex; flex-direction: column; gap: 0.5rem; max-height: calc(85vh - 2rem); min-height: 0; }
  .dialog h3 { margin: 0 0 0.3rem 0; font-size: 0.95rem; flex: 0 0 auto; }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    overflow-y: auto;
    min-height: 0;
    flex: 1 1 auto;
    padding-right: 0.2rem;
  }
  .fields > :global(*) { flex-shrink: 0; }
  label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  label.check { display: flex; gap: 0.4rem; align-items: center; text-transform: none; font-size: 0.8rem; color: var(--fg); }
  input:not([type='checkbox']) {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    min-width: 0;
  }
  .seg { display: flex; border: 1px solid var(--border); border-radius: 4px; overflow: hidden; }
  .seg-pick {
    flex: 1 1 0;
    font-size: 0.75rem;
    padding: 0.3rem 0.4rem;
    border: 0;
    border-right: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .seg-pick:last-child { border-right: 0; }
  .seg-pick.active { color: var(--fg); background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .host-row { display: flex; gap: 0.3rem; flex-wrap: wrap; max-height: 5.2rem; overflow-y: auto; }
  .host-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .host-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-pick:disabled { opacity: 0.4; cursor: not-allowed; }
  .pick-folder { align-self: flex-start; }
  .preview { margin: 0; font-size: 0.72rem; color: var(--fg-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .preview .k { text-transform: uppercase; font-size: 0.65rem; margin-right: 0.3rem; }
  .preview code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; align-items: center; flex: 0 0 auto; padding-top: 0.2rem; border-top: 1px solid var(--border); }
  .actions button, .pick-folder { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button.primary { border-color: var(--accent); }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
