<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { listHostWorktrees, type ProjectTreeRow, type WorktreeRow } from './projects';
  import { newSessionAbortable, sessions, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';
  import { slugifyBranch, finalizeBranchSlug } from './branch-slug';
  import { generateName, nameWords, tmuxNameSuffix } from './names';
  import Modal from './Modal.svelte';
  import PickerList from './PickerList.svelte';
  import type { PickerItem } from './PickerList.svelte';
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
    project,
    onCreate,
    onCancel,
    initialName,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
    /** Pre-fill the friendly name (the quick switcher's query). */
    initialName?: string;
  } = $props();

  // The project is fixed for the dialog's lifetime (the parent remounts for
  // a different one), so these snapshots are intentional.
  const projectId = untrack(() => project.project.id);
  const owner = untrack(() => project.project.owner);
  const repo = untrack(() => project.project.repo);
  const base = `dev-${owner}-${repo}`;

  // ── Remembered choices ───────────────────────────────────────────────
  // Global last host (existing pref) is the fallback; per-project memory
  // (`newsession.project.<id>`) wins so a project that always runs on
  // `hetzner` in a fresh worktree opens that way.
  const isString = (v: unknown): v is string => typeof v === 'string';
  interface ProjectMemory {
    host: string;
    /** Legacy (pre host-scoped picker): the local host's choice. */
    worktree?: number | 'new';
    /** Per host: worktree row id or 'new'. */
    worktrees?: Record<string, number | 'new'>;
    kind: 'work' | 'shell';
  }
  const isChoice = (v: unknown): v is number | 'new' => v === 'new' || typeof v === 'number';
  const isMemory = (v: unknown): v is ProjectMemory =>
    typeof v === 'object' &&
    v !== null &&
    typeof (v as ProjectMemory).host === 'string' &&
    ((v as ProjectMemory).worktree === undefined || isChoice((v as ProjectMemory).worktree)) &&
    ((v as ProjectMemory).worktrees == null ||
      (typeof (v as ProjectMemory).worktrees === 'object' &&
        Object.values((v as ProjectMemory).worktrees!).every(isChoice))) &&
    ((v as ProjectMemory).kind === 'work' || (v as ProjectMemory).kind === 'shell');
  const memoryKey = `newsession.project.${projectId}`;
  const memory = readPref<ProjectMemory | null>(memoryKey, null, (v): v is ProjectMemory | null => v === null || isMemory(v));
  /** The remembered choice for `host` (legacy flat value counts for local). */
  function rememberedFor(host: string): number | 'new' | undefined {
    return memory?.worktrees?.[host] ?? (host === 'local' ? memory?.worktree : undefined);
  }

  // A remembered host is only honoured while it is still pickable (visible,
  // and reachable unless it is `local`) — otherwise fall back to the global
  // last-host, then `local`. Mirrors the chip `disabled` rule below.
  function usableHost(alias: string | null | undefined): alias is string {
    return (
      !!alias &&
      $hosts.some((h) => h.alias === alias && !h.hidden && (h.reachable || h.alias === 'local'))
    );
  }
  let chosenHost = $state<string>(
    untrack(() => [memory?.host, readPref('last-host', '', isString)].find(usableHost) ?? 'local'),
  );
  $effect(() => {
    writePref('last-host', chosenHost);
  });

  // "work" runs Claude Code in the pane; "shell" runs a plain login shell.
  let chosenKind = $state<'work' | 'shell'>(untrack(() => memory?.kind ?? 'work'));
  // Optional command run on start for a shell session (empty = bare shell).
  let startCommand = $state<string>('');

  // Inverse of slugify-ish: take a worktree/branch name and produce a
  // sentence-cased label so the friendly-name field is pre-filled with
  // something readable when the user picks an existing worktree.
  function humanize(branch: string): string {
    if (!branch || branch === 'main') return '';
    const spaced = branch.replace(/[-_]+/g, ' ').trim();
    if (!spaced) return '';
    return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
  }

  // ── Generated names ──────────────────────────────────────────────────
  // Every slug already in use on this project: worktree names, the suffix
  // of every tmux name, and slugified friendly names. The generator avoids
  // all of them so "blue sirius" is never offered twice.
  const takenSlugs = $derived.by(() => {
    const set = new Set<string>();
    for (const w of hostWorktrees.rows) set.add(w.name.toLowerCase());
    for (const s of $sessions) {
      if (s.project_id !== project.project.id) continue;
      const suffix = tmuxNameSuffix(s.tmux_name, owner, repo);
      if (suffix) set.add(suffix.toLowerCase());
      if (s.friendly_name) set.add(finalizeBranchSlug(s.friendly_name));
    }
    return set;
  });
  const takenFriendly = $derived(
    new Set(
      $sessions
        .filter((s) => s.project_id === project.project.id && s.friendly_name)
        .map((s) => s.friendly_name!.trim().toLowerCase()),
    ),
  );

  function freshName(): string {
    return nameWords(generateName(takenSlugs));
  }

  // Default friendly name for a worktree pick: the humanised branch unless
  // it is empty (main) or already used by a session on this project — then
  // a generated pair, so the user never has to invent "work 3".
  function defaultFriendly(wt: WorktreeRow | null): string {
    const h = humanize(wt?.name ?? '');
    if (h && !takenFriendly.has(h.toLowerCase())) return h;
    return freshName();
  }

  // ── Worktree choice ──────────────────────────────────────────────────
  // The rows on offer are the CHOSEN HOST's: local answers from the project
  // tree synchronously; a remote host is scanned over SSH (`hostWorktrees`).
  type HostWorktreesState = {
    status: 'loading' | 'ready' | 'error';
    rows: WorktreeRow[];
    cloned: boolean;
    error?: string;
  };
  let hostWorktrees = $state<HostWorktreesState>(
    untrack(() => ({ status: 'ready', rows: project.worktrees, cloned: true })),
  );
  // A slow scan of the previous host must not land after a newer one.
  let scanSeq = 0;
  $effect(() => {
    const host = chosenHost;
    if (host === 'local') {
      // Only the local branch needs the live project tree; reading it here
      // instead of above the branch keeps this effect from tracking (and
      // being re-run by) the project's LOCAL worktrees while a REMOTE host
      // is what's actually selected. The backend only ever emits
      // `worktree:updated` / `worktree:removed` for local rows
      // (src-tauri/src/store/projects.rs:185), which is also what keeps a
      // remote scan's own upserts from feeding back into this effect.
      scanSeq++;
      hostWorktrees = { status: 'ready', rows: project.worktrees, cloned: true };
      return () => {
        scanSeq++;
      };
    }
    const seq = ++scanSeq;
    hostWorktrees = { status: 'loading', rows: [], cloned: true };
    // Never leave the previous host's row selected (and submittable) while
    // this scan is in flight, or if it errors, or never lands: start remote
    // hosts in new-worktree mode; the repair effect below corrects it once
    // real rows arrive. `untrack` because `onPickNew` reads
    // `nameDirty`/`takenSlugs` ($sessions) reactively, and this effect (which
    // fires an SSH call) must not re-run just because a session changed.
    untrack(() => onPickNew());
    void listHostWorktrees(host, projectId).then((r) => {
      if (seq !== scanSeq) return;
      if (!r.ok) {
        hostWorktrees = { status: 'error', rows: [], cloned: true, error: r.error.message };
        return;
      }
      if (!r.value || r.value.host_alias !== host) {
        // Shouldn't happen — the backend echoes the request's host_alias —
        // but never silently adopt a reply that isn't for the host this
        // scan was for; show an error instead of getting stuck on
        // "Scanning…" forever.
        hostWorktrees = {
          status: 'error',
          rows: [],
          cloned: true,
          error: `list_host_worktrees replied unexpectedly for ${host}`,
        };
        return;
      }
      hostWorktrees = {
        status: 'ready',
        rows: r.value.worktrees ?? [],
        cloned: r.value.cloned ?? true,
      };
    });
    // A dialog closed (or switched to another host) mid-scan must not let a
    // late reply land: bump the sequence so its `seq !== scanSeq` check fails.
    return () => {
      scanSeq++;
    };
  });

  function initialWorktree(): number | null {
    // A remembered (or default) REMOTE host's rows aren't known synchronously
    // — only `project.worktrees` (local) is available before the first scan
    // resolves. Start it in new-worktree mode rather than risk carrying over
    // a local row id that would be foreign (and rejected) on that host; the
    // scan-then-repair effects below correct this the moment real rows land.
    if (chosenHost !== 'local') return null;
    const remembered = rememberedFor(chosenHost);
    if (remembered === 'new') return null;
    if (typeof remembered === 'number' && project.worktrees.some((w) => w.id === remembered)) {
      return remembered;
    }
    return project.worktrees[0]?.id ?? null;
  }
  let chosenWorktreeId = $state<number | null>(untrack(initialWorktree));
  let inNewMode = $derived(chosenWorktreeId === null);
  let chosenWorktree = $derived(hostWorktrees.rows.find((w) => w.id === chosenWorktreeId) ?? null);

  // When the host's rows arrive (or change) — including landing on an
  // error, whose rows are always `[]` — keep the selection valid: the
  // remembered row for that host, else its `main`, else "+ new worktree".
  // Only skip this while a scan is actually in flight (`loading`); the
  // scan effect above already forces new-worktree mode for that window.
  $effect(() => {
    if (hostWorktrees.status === 'loading') return;
    const rows = hostWorktrees.rows;
    const current = untrack(() => chosenWorktreeId);
    if (current !== null && rows.some((w) => w.id === current)) return;
    const remembered = rememberedFor(untrack(() => chosenHost));
    const pick =
      (typeof remembered === 'number' && rows.find((w) => w.id === remembered)) ||
      rows.find((w) => w.name === 'main') ||
      (remembered === 'new' ? null : rows[0]) ||
      null;
    // `untrack`: `onPickWorktree`/`onPickNew` read `nameDirty`/`takenSlugs`
    // ($sessions) reactively, and this effect must not re-run (regenerating
    // the branch name under the user's cursor) just because a session event
    // fired.
    untrack(() => {
      if (pick) onPickWorktree(pick.id);
      else if (current !== null || !newWorktreeName) onPickNew();
    });
  });

  let newWorktreeName = $state<string>('');
  // Base branch to fork the new worktree from. Empty = the repo's default
  // branch (the backend falls back to default if the named branch is missing).
  let baseBranch = $state<string>('');
  // Track whether the user has manually edited the slug; once they do, we
  // stop auto-syncing it from the friendly-name field so their override
  // sticks. Reset on worktree-mode changes.
  let slugDirty = $state<boolean>(false);
  let friendlyName = $state<string>('');
  // A hand-edited tmux name sticks until the next mode/kind change; null
  // means "derived from the other fields" (the normal case).
  let nameOverride = $state<string | null>(null);
  // The user owns the friendly name once they type one (or the quick
  // switcher handed one over); worktree/mode switches then keep it instead
  // of regenerating. The dice clears it.
  let nameDirty = $state(false);

  // Initial fill (untracked: reads stores once, on open).
  untrack(() => {
    const wt = hostWorktrees.rows.find((w) => w.id === chosenWorktreeId) ?? null;
    if (initialName?.trim()) {
      friendlyName = initialName.trim();
      nameDirty = true;
    } else if (chosenWorktreeId === null) {
      friendlyName = freshName();
    } else {
      friendlyName = defaultFriendly(wt);
    }
    if (chosenWorktreeId === null) newWorktreeName = finalizeBranchSlug(friendlyName);
  });

  const friendlySlug = $derived(finalizeBranchSlug(friendlyName));
  const termSuffix = $derived(chosenKind === 'shell' ? '-term' : '');

  // The tmux name: `dev-<owner>-<repo>--<worktree>` (or the bare base for
  // main), and when that name is already live on the chosen host — a second
  // session on the same worktree — `…--<worktree>--<name-slug>` so the two
  // never collide. In new-worktree mode the slug IS the worktree name.
  // `.` and `:` are tmux target separators (the backend rejects them).
  const tmuxSafe = (s: string) => s.replace(/[.:]/g, '-');
  const takenOnHost = $derived(
    new Set($sessions.filter((s) => s.host_alias === chosenHost).map((s) => s.tmux_name)),
  );
  /** `stem` + suffix, or `stem-2`, `stem-3`… + suffix when that is live on the host. */
  function freeName(stem: string): string {
    if (!takenOnHost.has(stem + termSuffix)) return stem + termSuffix;
    for (let n = 2; ; n++) {
      const candidate = `${stem}-${n}${termSuffix}`;
      if (!takenOnHost.has(candidate)) return candidate;
    }
  }
  const derivedName = $derived.by(() => {
    if (inNewMode) {
      const slug = finalizeBranchSlug(newWorktreeName);
      return tmuxSafe(slug ? `${base}--${slug}` : base) + termSuffix;
    }
    const wt = chosenWorktree;
    const isMain = !wt || wt.name === 'main';
    const deterministic = tmuxSafe(isMain ? base : `${base}--${wt.name}`) + termSuffix;
    if (!takenOnHost.has(deterministic) || !friendlySlug) return deterministic;
    return freeName(tmuxSafe(isMain ? `${base}--${friendlySlug}` : `${base}--${wt.name}--${friendlySlug}`));
  });
  const name = $derived(nameOverride ?? derivedName);

  // Where the pane's cwd will be. Local paths come from the DB; remote ones
  // are derived like `remote_project_path` in service/sessions.rs: the host's
  // projects root (Settings → Projects, default `~/projects/github.com`) in
  // the configured layout. New worktrees land in whichever of `.worktrees` /
  // `.claude/worktrees` the repo already uses.
  const worktreeDir = $derived(
    (hostWorktrees.rows.length ? hostWorktrees.rows : project.worktrees).some((w) =>
      w.path.includes('/.claude/worktrees/'),
    )
      ? '.claude/worktrees'
      : '.worktrees',
  );
  const projectsLayout = $derived(settingLayout($fleetSettings));
  const remoteRoot = $derived(
    settingPathMap($fleetSettings, PROJECTS_RESOLVED_KEY)[chosenHost] ?? projectsDefaultRoot(projectsLayout),
  );
  onMount(() => {
    // The remote preview needs the backend's per-host roots; best effort.
    void loadFleetSettings();
  });
  const pathPreview = $derived.by(() => {
    const root =
      chosenHost === 'local' ? project.project.base_path : projectDir(remoteRoot, projectsLayout, owner, repo);
    if (inNewMode) {
      const slug = finalizeBranchSlug(newWorktreeName);
      return slug ? `${root}/${worktreeDir}/${slug}` : root;
    }
    const wt = chosenWorktree;
    if (!wt || wt.name === 'main') return root;
    // The row's own path is authoritative for local AND remote — deriving
    // one from `root` would hardcode the wrong layout for a repo that uses
    // `.worktrees/` instead of `.claude/worktrees/`.
    return wt.path;
  });

  const worktreeItems: PickerItem[] = $derived([
    ...(hostWorktrees.status === 'ready' && hostWorktrees.cloned ? hostWorktrees.rows : []).map((wt) => ({
      key: String(wt.id),
      label: wt.name,
      description: wt.branch && wt.branch !== wt.name ? wt.branch : undefined,
      meta: $sessions.some((s) => s.worktree_id === wt.id && s.status !== 'ghost') ? 'in use' : undefined,
      testid: 'worktree-row',
    })),
    { key: 'new', label: '+ new worktree', description: 'fresh branch from the base branch', testid: 'new-worktree-chip' },
  ]);
  const worktreeStatus = $derived.by((): string | null => {
    if (hostWorktrees.status === 'loading') return `Scanning ${chosenHost}…`;
    if (hostWorktrees.status === 'error') return `Couldn't list worktrees on ${chosenHost}: ${hostWorktrees.error}`;
    if (!hostWorktrees.cloned) return `Not cloned on ${chosenHost} yet — it is cloned on the first session.`;
    return null;
  });

  let busy = $state(false);
  let error: string | null = $state(null);
  let createController: AbortController | null = null;

  function onPickKind(kind: 'work' | 'shell') {
    chosenKind = kind;
    nameOverride = null;
  }

  function onPickWorktreeKey(key: string) {
    if (key === 'new') {
      onPickNew();
      return;
    }
    onPickWorktree(Number(key));
  }

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    newWorktreeName = '';
    baseBranch = '';
    slugDirty = false;
    nameOverride = null;
    const wt = hostWorktrees.rows.find((w) => w.id === id) ?? null;
    if (!nameDirty) friendlyName = defaultFriendly(wt);
  }

  function onPickNew() {
    chosenWorktreeId = null;
    baseBranch = '';
    slugDirty = false;
    nameOverride = null;
    if (!nameDirty) friendlyName = freshName();
    newWorktreeName = finalizeBranchSlug(friendlyName);
  }

  function reroll() {
    friendlyName = freshName();
    nameDirty = false;
    slugDirty = false;
    nameOverride = null;
    if (inNewMode) newWorktreeName = finalizeBranchSlug(friendlyName);
  }

  function onFriendlyNameInput(value: string) {
    friendlyName = value;
    // Clearing the field hands it back to the generator.
    nameDirty = value.trim() !== '';
    if (inNewMode && !slugDirty) {
      // Use the canonical branch-slug helper so the auto-derived slug is
      // git-safe the same way the user's direct edits are. Run it through
      // `finalizeBranchSlug` (not the live `slugifyBranch`) because the
      // friendly-name field's "word in progress" trailing space already
      // collapses to a stable form here — no point preserving a trailing
      // dash for a value the user isn't directly typing into the slug.
      newWorktreeName = finalizeBranchSlug(value);
    }
  }

  function onNewWorktreeNameInput(value: string) {
    // Auto-correct free-form input ("fix login bug") into a git-safe slug
    // ("fix-login-bug") as the user types.
    newWorktreeName = slugifyBranch(value);
    // Any divergence from the friendly-name-derived slug means the user has
    // taken manual control — stop auto-syncing future friendly-name edits.
    slugDirty = newWorktreeName !== finalizeBranchSlug(friendlyName);
  }

  function onNewWorktreeNameBlur() {
    const cleaned = finalizeBranchSlug(newWorktreeName);
    if (cleaned !== newWorktreeName) newWorktreeName = cleaned;
  }

  function onNameInput(value: string) {
    nameOverride = value;
  }

  /** Persist the host/worktree that were actually SUBMITTED (the caller
   *  snapshots these before the async create, since the host chips stay
   *  clickable while `busy`). Folds a legacy flat `worktree` value into the
   *  per-host map under `local` so it survives past the first create under
   *  the new shape instead of evaporating. */
  function remember(host: string, worktreeId: number | null) {
    const prev = readPref<ProjectMemory | null>(memoryKey, null, (v): v is ProjectMemory | null => v === null || isMemory(v));
    writePref<ProjectMemory>(memoryKey, {
      host,
      kind: chosenKind,
      worktrees: {
        ...(prev?.worktree !== undefined ? { local: prev.worktree } : {}),
        ...(prev?.worktrees ?? {}),
        [host]: worktreeId === null ? 'new' : worktreeId,
      },
    });
  }

  async function submit() {
    if (busy) return;
    if (inNewMode) {
      // Strip any trailing dash the live slugifier left in place so the
      // backend sees a fully-finalized branch name.
      const cleaned = finalizeBranchSlug(newWorktreeName);
      if (cleaned !== newWorktreeName) newWorktreeName = cleaned;
    }
    if (inNewMode && !newWorktreeName.trim()) {
      error = 'Worktree name required';
      return;
    }
    // Snapshot what is actually being submitted: the host chips (and, in
    // principle, the worktree picker) stay interactive while `busy`, so
    // `chosenHost`/`chosenWorktreeId` could change under us before the
    // request resolves. Remember what was submitted, not whatever is
    // current when the response lands.
    const submittedHost = chosenHost;
    const submittedWorktreeId = inNewMode ? null : chosenWorktreeId;
    busy = true;
    error = null;
    createController = new AbortController();
    const r = await newSessionAbortable(
      {
        host_alias: submittedHost,
        project_id: project.project.id,
        worktree_id: submittedWorktreeId,
        // An empty tmux name is legal: the backend mints one with the same
        // generator (see `fill_session_name`).
        name: name.trim(),
        new_worktree: inNewMode ? newWorktreeName.trim() || null : null,
        base_branch: inNewMode ? baseBranch.trim() || null : null,
        kind: chosenKind,
        start_command:
          chosenKind === 'shell' ? startCommand.trim() || null : null,
        friendly_name: friendlyName.trim() || null,
      },
      createController.signal,
    );
    createController = null;
    busy = false;
    if (!r.ok) {
      if (r.error.code !== 'E_CANCELLED') {
        error = r.error.message;
      }
      return;
    }
    remember(submittedHost, submittedWorktreeId);
    onCreate(r.value);
  }

  function cancelCreate() {
    createController?.abort();
  }

  // Cmd/Ctrl+R re-rolls the name. Bound on the window (capture phase) for
  // as long as the dialog is mounted, so it wins wherever focus sits —
  // including the <dialog> element itself — and never reloads the webview.
  function onWindowKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'r') {
      e.preventDefault();
      e.stopPropagation();
      reroll();
    }
  }
  onMount(() => {
    window.addEventListener('keydown', onWindowKeydown, true);
    return () => window.removeEventListener('keydown', onWindowKeydown, true);
  });

  // Enter in any field creates. Escape is handled by <Modal>.
  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === 'INPUT') {
        e.preventDefault();
        void submit();
      }
    }
  }
</script>

<Modal label="New session" onclose={onCancel} width="420px">
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog" onkeydown={onKeydown}>
  <h3>New session — {owner}/{repo}</h3>

  <div class="fields">
    <label for="friendly-name">Name</label>
    <div class="name-row">
      <input
        id="friendly-name"
        data-testid="friendly-name"
        data-autofocus
        value={friendlyName}
        oninput={(e) => onFriendlyNameInput((e.target as HTMLInputElement).value)}
        placeholder="blue sirius"
        maxlength="80"
      />
      <button
        type="button"
        class="dice"
        data-testid="reroll-name"
        onclick={reroll}
        title="Roll a new name (Ctrl/⌘+R)"
        aria-label="Roll a new name"
      >🎲</button>
    </div>

    <label for="kind-picker">Type</label>
    <div class="kind-row" id="kind-picker" role="group">
      <button
        class="kind-pick"
        class:active={chosenKind === 'work'}
        data-testid="kind-work"
        onclick={() => onPickKind('work')}
      >
        Claude
      </button>
      <button
        class="kind-pick"
        class:active={chosenKind === 'shell'}
        data-testid="kind-shell"
        onclick={() => onPickKind('shell')}
      >
        Shell
      </button>
    </div>

    {#if chosenKind === 'shell'}
      <label for="start-command">start command (optional)</label>
      <input
        id="start-command"
        data-testid="start-command"
        bind:value={startCommand}
        placeholder="e.g. pnpm test"
      />
    {/if}

    <label for="host-picker">Host</label>
    <div class="host-row" id="host-picker" role="group">
      {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
        <button
          class="host-pick"
          class:active={chosenHost === h.alias}
          disabled={!h.reachable && h.alias !== 'local'}
          onclick={() => {
            chosenHost = h.alias;
            nameOverride = null;
          }}
        >
          {h.alias}
        </button>
      {/each}
    </div>

    <label for="wt-picker">Worktree</label>
    {#if worktreeStatus}
      <p class="wt-status" data-testid="wt-status" class:err={hostWorktrees.status === 'error'}>{worktreeStatus}</p>
    {/if}
    <PickerList
      items={worktreeItems}
      activeKey={inNewMode ? 'new' : String(chosenWorktreeId)}
      onpick={onPickWorktreeKey}
      maxHeight="9rem"
      ariaLabel="Worktree"
      testid="wt-picker"
    />

    {#if inNewMode}
      <label for="new-wt-name">new branch / worktree name</label>
      <input
        id="new-wt-name"
        data-testid="new-worktree-name"
        value={newWorktreeName}
        oninput={(e) => onNewWorktreeNameInput((e.target as HTMLInputElement).value)}
        onblur={onNewWorktreeNameBlur}
        placeholder="fix login bug → fix-login-bug"
      />
      <label for="new-wt-base">base branch</label>
      <input
        id="new-wt-base"
        data-testid="new-worktree-base"
        bind:value={baseBranch}
        placeholder="default branch"
      />
    {/if}

    <label for="session-name">tmux name</label>
    <input
      id="session-name"
      data-testid="new-session-name"
      value={name}
      oninput={(e) => onNameInput((e.target as HTMLInputElement).value)}
      placeholder="empty = let fleet pick one"
    />
    <p class="preview" data-testid="path-preview" title={pathPreview}>
      <span class="k">cwd</span> <code>{pathPreview}</code>
    </p>

    {#if error}
      <p class="err">{error}</p>
    {/if}
  </div>

  <div class="actions">
    <span class="hint">↵ create · Ctrl/⌘R re-roll</span>
    <button onclick={onCancel} disabled={busy}>Cancel</button>
    {#if busy}
      <button type="button" data-testid="cancel-create" onclick={cancelCreate}>Cancel creation</button>
    {:else}
      <button class="primary" onclick={submit} disabled={inNewMode && !newWorktreeName.trim()}>Create</button>
    {/if}
  </div>
</div>
</Modal>

<style>
  /* The dialog owns its height budget: the field stack scrolls, the
     Create/Cancel row is pinned, so no number of worktrees or hosts can push
     the buttons off-screen. Modal's body caps at 85vh and adds 1rem padding
     per side; stay inside that. */
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
  /* Children keep their natural height (the worktree list and host chips
     are capped by their own max-height); when the window is short, .fields
     scrolls instead of squeezing them toward zero. :global so it reaches
     PickerList's root too. */
  .fields > :global(*) { flex-shrink: 0; }
  label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    min-width: 0;
  }
  .name-row { display: flex; gap: 0.3rem; }
  .name-row input { flex: 1 1 auto; }
  .dice {
    font-size: 1rem;
    line-height: 1;
    padding: 0.2rem 0.45rem;
    border: 1px solid var(--border);
    background: transparent;
    border-radius: 4px;
    cursor: pointer;
  }
  .dice:hover { border-color: var(--accent); }
  .host-row {
    display: flex;
    gap: 0.3rem;
    flex-wrap: wrap;
    max-height: 5.2rem;
    overflow-y: auto;
  }
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
  .kind-row { display: flex; gap: 0.3rem; }
  .kind-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.7rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .kind-pick.active { color: var(--fg); border-color: var(--accent); }
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
  .wt-status { font-size: 0.72rem; color: var(--fg-muted); margin: 0 0 0.2rem; }
  .wt-status.err { color: #e64a4a; }
  .actions {
    display: flex;
    gap: 0.4rem;
    justify-content: flex-end;
    align-items: center;
    flex: 0 0 auto;
    padding-top: 0.2rem;
    border-top: 1px solid var(--border);
  }
  .actions .hint { margin-right: auto; font-size: 0.68rem; color: var(--fg-muted); }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button.primary { border-color: var(--accent); }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
