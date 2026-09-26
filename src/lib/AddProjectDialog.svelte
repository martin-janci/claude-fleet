<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { addProject, confirmTokenOf, type AddProjectSource, type ProjectTreeRow } from './projects';
  import { defaultHost, hosts, isPickableHost } from './hosts';
  import { readPref, writePref } from './prefs';
  import { isComponent, parseRepoUrl } from './repo_url';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import GithubRepoBrowser from './GithubRepoBrowser.svelte';
  import AddProjectActions from './AddProjectActions.svelte';
  import SegmentedControl from './SegmentedControl.svelte';
  import HostChips from './HostChips.svelte';
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
    /** `host` is the host the project was actually added on (folder mode
     *  forces `local` whatever chip was chosen), so the follow-up session
     *  can open there. */
    onCreated: (row: ProjectTreeRow, host: string) => void;
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
  /** The mode was chosen by click/Enter (the GitHub browser takes focus)
   *  rather than arrowed onto (focus stays on the control). */
  let focusMode = $state(false);

  // ── Host ─────────────────────────────────────────────────────────────
  // Same chips and rule as NewSessionDialog: visible, and reachable unless
  // `local`. Folder mode is local-only; it overrides the choice without
  // touching it, so leaving folder mode restores the remembered host.
  const isString = (v: unknown): v is string => typeof v === 'string';
  const pickable = (alias: string) => isPickableHost($hosts, alias);
  let chosenHost = $state<string>(
    untrack(() => {
      const last = readPref('last-host', '', isString);
      return last && pickable(last) ? last : defaultHost($hosts);
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
  const ownerOk = $derived(isComponent(owner, 39));
  const repoOk = $derived(isComponent(repo, 100));
  // Field errors are shown only once the field has content; an empty field
  // just keeps Create disabled.
  const urlErr = $derived(
    url.trim() !== '' && !parsed
      ? 'Not a GitHub repository — use owner/repo, https://github.com/owner/repo or git@github.com:owner/repo.git'
      : null,
  );
  const ownerErr = $derived(
    owner !== '' && !ownerOk ? "Owner: 1–39 of A–Z a–z 0–9 . _ -, not starting with '-'" : null,
  );
  const repoErr = $derived(
    repo !== '' && !repoOk ? "Repository: 1–100 of A–Z a–z 0–9 . _ -, not starting with '-'" : null,
  );

  function source(): AddProjectSource | null {
    if (mode === 'clone') return parsed ? { kind: 'clone', url: url.trim() } : null;
    if (mode === 'folder') return folderPath ? { kind: 'folder', path: folderPath } : null;
    if (mode === 'new') return ownerOk && repoOk ? { kind: 'new', owner, repo, create_remote: createRemote } : null;
    return null; // github: pick a repository first
  }
  const canCreate = $derived(source() !== null);

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
    const target = mode === 'clone' ? parsed : mode === 'new' && ownerOk && repoOk ? { owner, repo } : null;
    return target ? projectDir(hostRoot, layout, target.owner, target.repo) : null;
  });

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
  let stopping = $state(false);
  /** What is in flight — drives the button label and the visible note. Not
   *  the request itself, so a confirmation token never sits in state. */
  let inflight = $state<{ host: string; kind: AddProjectSource['kind']; github: boolean } | null>(null);
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

  /** What stopping cannot undo: GitHub only for a `create_remote` run, the
   *  host only when it is remote; nothing for a local clone or folder. */
  const inflightNote = $derived.by((): string | null => {
    if (!inflight) return null;
    if (inflight.host === 'local') {
      return inflight.github ? 'Cancelling may come too late: the GitHub repository may already have been created.' : null;
    }
    const what = inflight.github
      ? 'creating the project and its GitHub repository'
      : inflight.kind === 'clone'
        ? 'the clone'
        : 'creating the project';
    return `${inflight.host} may still finish ${what} after you stop waiting.`;
  });

  async function submit() {
    if (busy || pendingConfirm) return;
    const s = source();
    if (s) await run(host, s);
  }

  async function run(h: string, s: AddProjectSource) {
    busy = true;
    stopping = false;
    inflight = { host: h, kind: s.kind, github: s.kind === 'new' && s.create_remote };
    error = null;
    controller = new AbortController();
    const r = await addProject(h, s, controller.signal);
    controller = null;
    busy = false;
    stopping = false;
    inflight = null;
    if (destroyed) return;
    if (r.ok) {
      onCreated(r.value, h);
      return;
    }
    const remote = s.kind === 'new' && s.create_remote;
    const confirmed = s.kind === 'new' && s.confirm !== undefined;
    if (r.error.code === 'E_CONFIRM_REQUIRED') {
      const token = confirmTokenOf(r.error);
      if (s.kind === 'new' && remote && token && !confirmed) {
        // Never retried automatically: only the confirmation's own button
        // resends, with exactly this token and this frozen request.
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
    if (stopping) return;
    stopping = true;
    controller?.abort();
  }

  // Enter in a field creates. Escape is handled by <Modal>.
  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.shiftKey || e.altKey) return;
    if ((e.target as HTMLElement | null)?.tagName === 'INPUT' && (e.target as HTMLInputElement).type !== 'checkbox') {
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
    <SegmentedControl
      options={MODES}
      value={mode}
      label="Source"
      testidPrefix="add-mode-"
      disabled={busy}
      onchange={(id, via) => {
        focusMode = via === 'click';
        mode = id;
      }}
    />

    <HostChips
      active={host}
      labelId="add-host-label"
      disabled={busy}
      lockedReason={(alias) =>
        mode === 'folder' && alias !== 'local'
          ? 'An existing folder can only be adopted on local — it is picked on this machine'
          : null}
      onpick={(alias) => (chosenHost = alias)}
    />

    {#if mode === 'clone'}
      <label for="add-url">Repository</label>
      <input
        id="add-url"
        data-testid="clone-url"
        data-autofocus
        bind:value={url}
        disabled={busy}
        aria-invalid={urlErr ? 'true' : undefined}
        aria-describedby={urlErr ? 'add-url-err' : undefined}
        placeholder="owner/repo or https://github.com/owner/repo"
      />
      {#if urlErr}<p class="err" id="add-url-err" data-testid="add-url-err">{urlErr}</p>{/if}
    {:else if mode === 'github'}
      <span class="label">Repositories on {host}</span>
      <GithubRepoBrowser {host} onpick={pickGithubRepo} autofocus={focusMode} />
    {:else if mode === 'folder'}
      <span class="label">Folder (an existing git checkout)</span>
      <button type="button" class="pick-folder" data-testid="choose-folder" disabled={busy} onclick={chooseFolder}>
        Choose folder…
      </button>
    {:else}
      <label for="new-owner">Owner</label>
      <input
        id="new-owner"
        data-testid="new-owner"
        data-autofocus
        bind:value={owner}
        disabled={busy}
        aria-invalid={ownerErr ? 'true' : undefined}
        aria-describedby={ownerErr ? 'add-owner-err' : undefined}
        maxlength="39"
        placeholder="martin-janci"
      />
      {#if ownerErr}<p class="err" id="add-owner-err" data-testid="add-owner-err">{ownerErr}</p>{/if}
      <label for="new-repo">Repository name</label>
      <input
        id="new-repo"
        data-testid="new-repo"
        bind:value={repo}
        disabled={busy}
        aria-invalid={repoErr ? 'true' : undefined}
        aria-describedby={repoErr ? 'add-repo-err' : undefined}
        maxlength="100"
        placeholder="my-project"
      />
      {#if repoErr}<p class="err" id="add-repo-err" data-testid="add-repo-err">{repoErr}</p>{/if}
      <label class="check">
        <input type="checkbox" data-testid="new-create-remote" bind:checked={createRemote} disabled={busy} />
        Also create a private repository on GitHub
      </label>
    {/if}

    {#if pathPreview}
      <p class="preview" data-testid="add-path-preview" title={pathPreview}>
        <span class="k">{mode === 'folder' ? 'folder' : 'into'}</span> <code>{pathPreview}</code>
      </p>
    {/if}
    {#if error}
      <p class="err" role="alert" data-testid="add-error">{error}</p>
    {/if}
  </div>

  <AddProjectActions
    {busy}
    {stopping}
    remote={inflight !== null && inflight.host !== 'local'}
    note={inflightNote}
    {canCreate}
    oncreate={submit}
    onclose={onCancel}
    onstop={cancelCreate}
  />
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
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-height: calc(85vh - 2rem);
    min-height: 0;
  }
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
  label, .label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  label.check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    text-transform: none;
    font-size: 0.8rem;
    color: var(--fg);
  }
  input:not([type='checkbox']) {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    min-width: 0;
  }
  .pick-folder {
    align-self: flex-start;
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .preview {
    margin: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .preview .k { text-transform: uppercase; font-size: 0.65rem; margin-right: 0.3rem; }
  .preview code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
</style>
