<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import {
    addProject,
    confirmTokenOf,
    mergeProject,
    projects,
    type AddProjectSource,
    type ProjectTreeRow,
  } from './projects';
  import { setProjectPick } from './project_picks';
  import { orgs, loadOrgs, addOrgRule } from './orgs';
  import { trackers, loadTrackers, updateTracker } from './trackers';
  import { push } from './toasts';
  import { coveringOrgId, githubTrackers, placeProject, reposWith } from './add_project_place';
  import type { IpcError } from './result';
  import { defaultHost, hosts, isPickableHost } from './hosts';
  import { hubStatus } from './hub';
  import { readPref, writePref } from './prefs';
  import { clearWizard, getWizard, saveWizard, startedWhere, type WizardState } from './wizard_state';
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
    initialCloneUrl,
    initialMode,
    blocked = null,
    saveDelayMs = 800,
  }: {
    /** `host` is the host the project was actually added on (folder mode
     *  forces `local` whatever chip was chosen), so the follow-up session
     *  can open there. */
    onCreated: (row: ProjectTreeRow, host: string) => void;
    onCancel: () => void;
    /** Prefills the Clone URL field and starts in that mode (the switcher's
     *  Add row passes what was typed). */
    initialCloneUrl?: string;
    /** The source to open on. Default: From GitHub, as on the AddProject
     *  board; Clone a URL when `initialCloneUrl` is given. */
    initialMode?: 'github' | 'clone' | 'folder' | 'new';
    /** Why Add project cannot work from this window right now (a hub client
     *  whose link is down); disables Create and says why. */
    blocked?: string | null;
    /** How long the fields must be still before they are saved for a
     *  resume on another device, ms; injectable for tests. */
    saveDelayMs?: number;
  } = $props();

  // ── Modes ────────────────────────────────────────────────────────────
  // The four sources of the AddProject board (redesign 6.11), in its order.
  type Mode = 'clone' | 'github' | 'folder' | 'new';
  const ALL_MODES: { id: Mode; label: string }[] = [
    { id: 'github', label: 'From GitHub' },
    { id: 'clone', label: 'Clone a URL' },
    { id: 'folder', label: 'Existing folder' },
    { id: 'new', label: 'New repo' },
  ];
  // On a hub client `local` is the hub's machine, and the folder picker is
  // this machine's — so an existing folder cannot be offered there.
  const MODES = $derived($hubStatus.remote ? ALL_MODES.filter((m) => m.id !== 'folder') : ALL_MODES);
  let mode = $state<Mode>(
    untrack(() => {
      const m = initialMode ?? (initialCloneUrl !== undefined ? 'clone' : 'github');
      return m === 'folder' && $hubStatus.remote ? 'github' : m;
    }),
  );
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
  /** Clone a URL / From GitHub: more hosts to clone onto after `host` (the
   *  board's "Clone on" with several). Each gets the checkout a session
   *  there would otherwise clone on first use. `local` is never one: the
   *  project's own row lives there. */
  let alsoHosts = $state<string[]>([]);
  const multiHost = $derived(mode === 'clone' || mode === 'github');
  const alsoChoices = $derived(
    $hosts.filter((h) => !h.hidden && h.reachable && h.alias !== 'local' && h.alias !== host).map((h) => h.alias),
  );
  const extraHosts = $derived(multiHost ? alsoHosts.filter((a) => alsoChoices.includes(a)) : []);
  function toggleAlso(alias: string) {
    alsoHosts = alsoHosts.includes(alias) ? alsoHosts.filter((a) => a !== alias) : [...alsoHosts, alias];
  }
  /** Whether stopping cannot reach the run: any host but this machine's own
   *  `local`. On a hub client every host is remote, `local` included. */
  const hostIsRemote = (h: string) => h !== 'local' || $hubStatus.remote;

  // ── Fields ───────────────────────────────────────────────────────────
  let url = $state(untrack(() => initialCloneUrl ?? ''));
  let folderPath = $state<string | null>(null);
  let owner = $state('');
  let repo = $state('');
  let createRemote = $state(false);
  /** From GitHub: the owner listed (empty = the host login's own) and the
   *  ticked repositories, each added as a clone. */
  let ghOwner = $state('');
  let ghOwnerShown = $state('');
  let ghSelected = $state<string[]>([]);
  const inFleet = (nameWithOwner: string) => {
    const k = nameWithOwner.toLowerCase();
    return $projects.some((p) => `${p.project.owner}/${p.project.repo}`.toLowerCase() === k);
  };
  /** Owners the fleet already holds projects of: the owner field's choices. */
  const knownOwners = $derived(
    [...new Set($projects.filter((p) => !p.project.system).map((p) => p.project.owner))].sort((a, b) =>
      a.localeCompare(b),
    ),
  );
  // ── Organisation and Tracker (the board's two placement fields) ─────
  onMount(() => {
    // Best effort: without them the two fields are simply not offered.
    if ($orgs.length === 0) void loadOrgs();
    if ($trackers.length === 0) void loadTrackers();
  });
  /** `undefined` until the person picks: then the org the first repository
   *  already belongs to is shown, and nothing new is written for it. */
  let orgPick = $state<number | null | undefined>(undefined);
  let trackerId = $state<number | null>(null);
  const ghTrackers = $derived(githubTrackers($trackers));
  const tracker = $derived(ghTrackers.find((t) => t.id === trackerId) ?? null);
  /** The first `owner/repo` the verb would add, for the defaults and notes. */
  const firstTarget = $derived.by((): { owner: string; repo: string } | null => {
    if (mode === 'github') {
      const n = ghSelected.find((x) => !inFleet(x));
      const [o, r] = n ? n.split('/') : [];
      return o && r ? { owner: o, repo: r } : null;
    }
    if (mode === 'clone') return parsed;
    if (mode === 'new') return ownerOk && repoOk ? { owner, repo } : null;
    return null;
  });
  const orgId = $derived(
    orgPick !== undefined ? orgPick : firstTarget ? coveringOrgId($orgs, firstTarget.owner, firstTarget.repo) : null,
  );
  const trackerNote = $derived.by((): string | null => {
    if (!tracker || !firstTarget) return null;
    return reposWith(tracker, firstTarget.owner, firstTarget.repo) === null
      ? `${tracker.name} already covers it.`
      : `Its issues will sync from ${tracker.name}.`;
  });

  function toggleRepo(name: string) {
    ghSelected = ghSelected.includes(name) ? ghSelected.filter((n) => n !== name) : [...ghSelected, name];
  }
  /** List another owner's repositories (on Enter or leaving the field). */
  function applyOwner() {
    const o = ghOwner.trim();
    if (o === ghOwnerShown) return;
    ghOwnerShown = o;
    ghSelected = [];
  }

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
    return null; // github: see `sources`
  }
  /** Everything the verb adds: each ticked repository From GitHub, else the
   *  one source the other modes build. */
  function sources(): AddProjectSource[] {
    if (mode === 'github') return ghSelected.filter((n) => !inFleet(n)).map((n) => ({ kind: 'clone', url: n }));
    const s = source();
    return s ? [s] : [];
  }
  const pending = $derived(sources().length);
  const canCreate = $derived(pending > 0 && !blocked);
  /** The board's verb: "Add project", or "Add 2 projects" From GitHub. */
  const verb = $derived(mode === 'github' && pending > 1 ? `Add ${pending} projects` : 'Add project');
  /** The footer's summary From GitHub ("2 repos · on mefistos"). */
  const summary = $derived.by((): string | null => {
    if (pending === 0 || (mode !== 'github' && extraHosts.length === 0)) return null;
    const on = [host, ...extraHosts];
    return `${pending} ${pending === 1 ? 'repo' : 'repos'} · on ${on.join(', ')}`;
  });

  // ── Destination preview ──────────────────────────────────────────────
  // The preview needs the backend's per-host roots, and on a hub client
  // those are the hub's: `get_fleet_settings` is local-only there, so this
  // machine would preview its own (default) roots. No preview beats a wrong
  // one — the hub answers with the real path.
  onMount(() => {
    // Best effort.
    if (!$hubStatus.remote) void loadFleetSettings();
  });
  const layout = $derived(settingLayout($fleetSettings));
  const hostRoot = $derived(
    settingPathMap($fleetSettings, PROJECTS_RESOLVED_KEY)[host] ?? projectsDefaultRoot(layout),
  );
  const pathPreview = $derived.by((): string | null => {
    if ($hubStatus.remote) return null;
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

  // ── Resume on another device (gap plan G7.2) ────────────────────────
  // What the person typed is kept on the hub as the `add_project` wizard,
  // so the dialog opened on another of their devices offers to carry on.
  // A dialog opened with a prefill (the switcher's Add row) starts fresh
  // and offers nothing. The draft goes once a project is added or the
  // person chooses Start over; Cancel keeps it.
  const prefilled = untrack(() => initialCloneUrl !== undefined || initialMode !== undefined);
  let resumable = $state<WizardState | null>(null);
  /** No save until the offer is answered (or there is none), so a dialog
   *  opened here never overwrites what the other device kept. */
  let resumeSettled = $state(untrack(() => prefilled));
  onMount(() => {
    if (prefilled) return;
    void getWizard('add_project').then((r) => {
      if (destroyed) return;
      if (r.ok && r.value) resumable = r.value;
      else resumeSettled = true;
    });
  });
  const answers = $derived<Record<string, unknown>>({
    mode,
    host: chosenHost,
    ...(url.trim() ? { url: url.trim() } : {}),
    ...(owner ? { owner } : {}),
    ...(repo ? { repo } : {}),
    ...(createRemote ? { create_remote: true } : {}),
    ...(ghOwner.trim() ? { gh_owner: ghOwner.trim() } : {}),
    ...(ghSelected.length ? { gh_selected: ghSelected } : {}),
    ...(alsoHosts.length ? { also_hosts: alsoHosts } : {}),
    ...(orgPick !== undefined ? { org_id: orgPick } : {}),
    ...(trackerId != null ? { tracker_id: trackerId } : {}),
  });
  /** Worth keeping: something to add has been typed or ticked. */
  const worthKeeping = $derived(url.trim() !== '' || owner !== '' || repo !== '' || ghSelected.length > 0);
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const snapshot = JSON.stringify(answers);
    const keep = worthKeeping && resumeSettled && !busy;
    const label = firstTarget ? `${firstTarget.owner}/${firstTarget.repo}` : undefined;
    clearTimeout(saveTimer);
    if (!keep) return;
    saveTimer = setTimeout(() => void saveWizard('add_project', 1, JSON.parse(snapshot), { label }), saveDelayMs);
  });
  onMount(() => () => clearTimeout(saveTimer));

  function resume(w: WizardState) {
    const a = w.answers ?? {};
    const str = (v: unknown) => (typeof v === 'string' ? v : '');
    const m = str(a.mode);
    if (MODES.some((x) => x.id === m)) mode = m as Mode;
    if (str(a.host) && pickable(str(a.host))) chosenHost = str(a.host);
    url = str(a.url);
    owner = str(a.owner);
    repo = str(a.repo);
    createRemote = a.create_remote === true;
    ghOwner = str(a.gh_owner);
    ghOwnerShown = ghOwner;
    ghSelected = Array.isArray(a.gh_selected) ? a.gh_selected.filter((x): x is string => typeof x === 'string') : [];
    alsoHosts = Array.isArray(a.also_hosts) ? a.also_hosts.filter((x): x is string => typeof x === 'string') : [];
    if (a.org_id === null || typeof a.org_id === 'number') orgPick = a.org_id;
    if (typeof a.tracker_id === 'number') trackerId = a.tracker_id;
    resumable = null;
    resumeSettled = true;
  }

  function startOver() {
    resumable = null;
    resumeSettled = true;
    void clearWizard('add_project');
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
    if (!hostIsRemote(inflight.host)) {
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
    if (busy || pendingConfirm || blocked) return;
    if (mode === 'github') {
      await runMany(host, sources());
      return;
    }
    const s = source();
    if (s) await run(host, s);
  }

  /** From GitHub: add each ticked repository in turn. The first failure
   *  stops the rest and is shown; what was already added leaves the
   *  selection (it now reads "already in fleet"), so Add again carries on
   *  with what is left. */
  async function runMany(h: string, list: AddProjectSource[]) {
    const added: ProjectTreeRow[] = [];
    for (const s of list) {
      const row = await run(h, s, false);
      if (destroyed) return;
      if (!row) {
        if (added.length > 0 && error) {
          const names = added.map((r) => `${r.project.owner}/${r.project.repo}`).join(', ');
          error = `Added ${names}. ${error}`;
        }
        return;
      }
      added.push(row);
      mergeProject(row);
      if (s.kind === 'clone') ghSelected = ghSelected.filter((n) => n !== s.url);
    }
    if (added.length === 0) return;
    await finish(
      added.map((row, i) => ({ row, source: list[i] })),
      h,
    );
  }

  /** After the adds: the other hosts, the organisation and the tracker,
   *  then the dialog hands the first project back. What the extras could
   *  not do is said in a toast: the projects are in the fleet either way. */
  async function finish(done: { row: ProjectTreeRow; source: AddProjectSource }[], h: string) {
    const problems: string[] = [];
    busy = true;
    for (const { row, source } of done) {
      if (source.kind !== 'clone') continue;
      for (const also of extraHosts) {
        inflight = { host: also, kind: 'clone', github: false };
        controller = new AbortController();
        const r = await addProject(also, { ...source, existing: true }, controller.signal);
        controller = null;
        if (destroyed) return;
        if (!r.ok) {
          const name = `${row.project.owner}/${row.project.repo}`;
          problems.push(
            r.error.code === 'E_EXISTS'
              ? `${name} was not cloned on ${also}: this hub can't add a second host yet. It is cloned there when a session starts.`
              : `${name} was not cloned on ${also}: ${r.error.message}`,
          );
        }
      }
    }
    for (const { row } of done) {
      problems.push(
        ...(await placeProject(row.project, { orgId, tracker }, $orgs, {
          addOrgRule: (rule) => addOrgRule(rule),
          updateTracker: (id, opts) => updateTracker(id, opts),
        })),
      );
    }
    busy = false;
    inflight = null;
    if (destroyed) return;
    for (const message of problems) push({ kind: 'error', message });
    // The host answers for the first; the others are kept in the New
    // session picker the same way (onProjectAdded keeps the first).
    for (const { row } of done.slice(1)) {
      void setProjectPick(row.project.owner, row.project.repo, { vis: 'keep' }, { quiet: true });
    }
    clearTimeout(saveTimer);
    void clearWizard('add_project');
    onCreated(done[0].row, h);
  }

  /** Adds one source. Hands the row back instead of reporting it when
   *  `report` is off (From GitHub reports once, after the last). */
  async function run(h: string, s: AddProjectSource, report = true): Promise<ProjectTreeRow | null> {
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
    if (destroyed) return null;
    if (r.ok) {
      if (report) await finish([{ row: r.value, source: s }], h);
      return r.value;
    }
    fail(h, s, r.error);
    return null;
  }

  function fail(h: string, s: AddProjectSource, e: IpcError) {
    const r = { error: e };
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
          ? 'The GitHub confirmation expired or was already used — press Add project to confirm again.'
          : r.error.message;
      }
      return;
    }
    if (r.error.code === 'E_CANCELLED') {
      // A cancelled GitHub creation may have happened anyway: the backend's
      // message says so, and it must not be swallowed.
      error = remote ? r.error.message : !hostIsRemote(h) ? 'Cancelled.' : `Stopped waiting — ${h} may still finish.`;
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
  <header>
    <h3>Add project</h3>
    <p class="lead" data-testid="add-lead">
      Bring a repository into the fleet so sessions can start in it.
    </p>
  </header>

  {#if resumable}
    <div class="resume" role="status" data-testid="add-resume">
      <span
        >You started adding <strong>{resumable.label ?? 'a project'}</strong>
        {startedWhere(resumable, Math.floor(Date.now() / 1000))}.</span
      >
      <button type="button" class="btn btn--primary" data-testid="add-resume-go" onclick={() => resumable && resume(resumable)}>Resume</button>
      <button type="button" class="btn btn--quiet" data-testid="add-resume-over" onclick={startOver}>Start over</button>
    </div>
  {/if}

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
    {#if multiHost && alsoChoices.length > 0}
      <span class="label" id="add-also-label">Also clone on</span>
      <div class="also-row" role="group" aria-labelledby="add-also-label">
        {#each alsoChoices as a (a)}
          <button
            type="button"
            class="btn btn--chip btn--toggle tag--mono"
            data-testid="add-also-host"
            data-alias={a}
            aria-pressed={alsoHosts.includes(a)}
            disabled={busy}
            onclick={() => toggleAlso(a)}>{a}</button
          >
        {/each}
      </div>
    {/if}

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
      <label for="gh-owner">Owner</label>
      <input
        id="gh-owner"
        type="text"
        data-testid="gh-owner"
        list="gh-owner-known"
        bind:value={ghOwner}
        disabled={busy}
        maxlength="39"
        placeholder="Your repositories, or an organisation"
        onchange={applyOwner}
        onkeydown={(e) => {
          if (e.key !== 'Enter') return;
          // Enter lists this owner; it does not add anything.
          e.preventDefault();
          e.stopPropagation();
          applyOwner();
        }}
      />
      <datalist id="gh-owner-known">
        {#each knownOwners as o (o)}<option value={o}></option>{/each}
      </datalist>
      <span class="label">Repositories on {host}</span>
      <GithubRepoBrowser
        {host}
        owner={ghOwnerShown}
        selected={ghSelected}
        ontoggle={toggleRepo}
        {inFleet}
        disabled={busy}
        autofocus={focusMode}
      />
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

    {#if mode !== 'folder' && $orgs.length > 0}
      <label for="add-org">Organisation</label>
      <select
        id="add-org"
        data-testid="add-org"
        disabled={busy}
        value={orgId === null ? '' : String(orgId)}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          orgPick = v === '' ? null : Number(v);
        }}
      >
        <option value="">None</option>
        {#each $orgs as o (o.id)}<option value={String(o.id)}>{o.name}</option>{/each}
      </select>
    {/if}
    {#if mode !== 'folder' && ghTrackers.length > 0}
      <label for="add-tracker">Tracker</label>
      <select
        id="add-tracker"
        data-testid="add-tracker"
        disabled={busy}
        value={trackerId === null ? '' : String(trackerId)}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          trackerId = v === '' ? null : Number(v);
        }}
      >
        <option value="">None</option>
        {#each ghTrackers as t (t.id)}<option value={String(t.id)}>{t.name}</option>{/each}
      </select>
      {#if trackerNote}<p class="preview" data-testid="add-tracker-note">{trackerNote}</p>{/if}
    {/if}
    {#if summary}
      <p class="preview" data-testid="add-summary">{summary}</p>
    {/if}
    {#if pathPreview}
      <p class="preview" data-testid="add-path-preview" title={pathPreview}>
        <span class="k">{mode === 'folder' ? 'folder' : 'into'}</span> <code>{pathPreview}</code>
      </p>
    {/if}
    {#if blocked}
      <p class="err" role="alert" data-testid="add-blocked">{blocked}</p>
    {/if}
    {#if error}
      <p class="err" role="alert" data-testid="add-error">{error}</p>
    {/if}
  </div>

  <AddProjectActions
    {busy}
    {stopping}
    remote={inflight !== null && hostIsRemote(inflight.host)}
    note={inflightNote}
    {canCreate}
    {verb}
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
  header { display: flex; flex-direction: column; gap: var(--space-1); flex: 0 0 auto; }
  .dialog h3 { margin: 0; font-size: var(--text-lg); font-weight: var(--text-lg-weight); }
  .lead { margin: 0 0 0.3rem 0; color: var(--fg-muted); font-size: var(--text-sm); }
  .resume {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
  }
  .resume > span { flex: 1 1 12rem; }
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
  label, .label { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; }
  label.check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    text-transform: none;
    font-size: var(--text-2xs);
    color: var(--fg);
  }
  input:not([type='checkbox']) {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    min-width: 0;
  }
  .also-row { display: flex; flex-wrap: wrap; gap: 0.3rem; }
  select {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
  }
  .pick-folder {
    align-self: flex-start;
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .preview {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .preview .k { text-transform: uppercase; font-size: var(--text-2xs); margin-right: 0.3rem; }
  .preview code { font-family: var(--font-mono); }
  .err { color: var(--danger); font-size: var(--text-2xs); margin: 0; }
</style>
