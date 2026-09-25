<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import { listHostWorktrees, projects, type ProjectTreeRow, type WorktreeRow } from './projects';
  import { extractWorkKey, keyFromTicketUrl, workKeyFor, worktreeBranchById } from './work_keys';
  import { endedWorkLinks, pastWorkSummary, type WorkLink } from './work';
  import ResumeDialog from './ResumeDialog.svelte';
  import { selectSessionExplicitly } from './selection';
  import { newSessionAbortable, sessions, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';
  import { slugifyBranch, finalizeBranchSlug } from './branch-slug';
  import { generateName, nameWords, tmuxNameSuffix } from './names';
  import Modal from './Modal.svelte';
  import PickerList from './PickerList.svelte';
  import HostChips from './HostChips.svelte';
  import { refreshAccountUsage } from './account_usage_store';
  import { push, pushError } from './toasts';
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
  import { hubStatus, ownsTheFleet, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { startWork, ticketBriefPreview, type TicketRow } from './trackers';
  import { startWorkMulti, siblingCandidates, multiStartNote } from './multi_start';

  let {
    project,
    onCreate,
    onCancel,
    initialName,
    initialHost,
    ticket,
    clock = () => Math.floor(Date.now() / 1000),
    locale,
    timeZone,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
    /** Pre-fill the friendly name (the quick switcher's query). */
    initialName?: string;
    /** Preselect this host (e.g. where Add project just put the project);
     *  wins over the remembered choices while it is pickable. */
    initialHost?: string;
    /** Start work on this ticket (work graph M3): the dialog offers "Brief
     *  Claude with the ticket" with an editable preview, and creating goes
     *  through `start_work`, which links the session `started`. */
    ticket?: TicketRow;
    /** Unix seconds for the host chips' usage wording; injectable for tests. */
    clock?: () => number;
    locale?: string;
    timeZone?: string;
  } = $props();

  // One coarse clock for the chips' "resets 15:10" / "2 min ago" wording.
  const readClock = () => clock();
  let now = $state(readClock());
  $effect(() => {
    const t = setInterval(() => (now = readClock()), 30_000);
    return () => clearInterval(t);
  });

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
  // last-host, then `local`. Mirrors the chip `disabled` rule in HostChips.
  function usableHost(alias: string | null | undefined): alias is string {
    return (
      !!alias &&
      $hosts.some((h) => h.alias === alias && !h.hidden && (h.reachable || h.alias === 'local'))
    );
  }
  let chosenHost = $state<string>(
    untrack(
      () => [initialHost, memory?.host, readPref('last-host', '', isString)].find(usableHost) ?? 'local',
    ),
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
  // tree synchronously; a remote host is scanned over SSH (`hostWorktrees`)
  // — by this app when it owns the fleet, by the hub when it does not.
  type HostWorktreesState = {
    // `unlistable`: nobody could run the scan (see `HUB_CANNOT_SCAN`), which
    // is a different thing from the scan failing — no rows, and no error to
    // report about the host itself.
    status: 'loading' | 'ready' | 'error' | 'unlistable';
    rows: WorktreeRow[];
    cloned: boolean;
    error?: string;
  };
  // Error codes that mean the HUB could not answer the scan at all: it is
  // older than this app and serves no such tool, its wire contract is out of
  // range, or it is unreachable / unusable this launch. None of them says
  // anything about the host, so the dialog degrades to what it did before
  // the hub had the tool (#146) instead of showing a failure. Matched by
  // code, never by message text.
  //
  // `E_FORBIDDEN` is in the list, and it is safe to read as "this hub does
  // not know the tool" for THIS call specifically: `list_host_worktrees` is
  // client-callable and readonly in the policy table of every hub that serves
  // it, so neither of the hub's two gates can refuse a paired client for it —
  // and both gates fail closed on the tool NAME, so a hub with no row for it
  // refuses with "not a client-callable tool" rather than "no such tool". A
  // policy refusal for this one tool therefore means the name is unknown
  // there. `E_HUB_PROTOCOL` stays for the callers those gates let through to
  // the router: a master token, or a hub that predates the fail-closed gate.
  // It also covers an argument-binding disagreement with a NEWER hub, which
  // is knowingly folded in here: #166's contract gate catches the revision
  // skew that would cause one, and the arguments are this command's own
  // struct rather than a hand-written literal.
  const HUB_CANNOT_SCAN = [
    'E_FORBIDDEN',
    'E_HUB_PROTOCOL',
    'E_HUB_CONTRACT',
    'E_HUB_UNREACHABLE',
    'E_HUB_UNAVAILABLE',
    // A scan the hub never answered says nothing about the host's worktrees
    // either; the red error line would blame the scan for a slow hub.
    'E_HUB_TIMEOUT',
  ];
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
    // A hub client gets here too: `list_host_worktrees` routes to the hub,
    // which has the SSH route this app does not (#168). What must never
    // happen — and did, before #146 — is reading `project.worktrees` as a
    // substitute: `list_projects_joined` LEFT JOINs worktrees on
    // `host_alias = 'local'` only, `upsert_worktree_on` fires
    // `worktree:updated` for local rows only, and the `list_worktrees` tool
    // goes through `list_worktrees_for_project`, also local-only (all in
    // `crates/fleet-core/src/store/projects.rs`). So `project.worktrees` is
    // always the STORE's own local checkout and never a remote host's —
    // filtering it by `host_alias === host` for a non-local `host` always
    // came back empty, which silently read as "this host has no worktrees"
    // rather than "unknown".
    const seq = ++scanSeq;
    hostWorktrees = { status: 'loading', rows: [], cloned: true };
    // Never leave the previous host's row selected (and submittable) while
    // this scan is in flight, or if it errors, or never lands: force
    // new-worktree mode; the repair effect below corrects it once real rows
    // arrive. Only when a row is actually selected — if the user was
    // already mid-new-worktree (typed a branch name, a base branch) that
    // in-progress input must survive the host switch, not get discarded.
    // `untrack` because `onPickNew` reads `nameDirty`/`takenSlugs`
    // ($sessions) reactively, and this effect (which fires an SSH call)
    // must not re-run just because a session changed.
    untrack(() => {
      if (chosenWorktreeId !== null) onPickNew();
    });
    void listHostWorktrees(host, projectId).then((r) => {
      if (seq !== scanSeq) return;
      if (!r.ok) {
        hostWorktrees = HUB_CANNOT_SCAN.includes(r.error.code)
          ? { status: 'unlistable', rows: [], cloned: true }
          : { status: 'error', rows: [], cloned: true, error: r.error.message };
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
    // A ticket start works on its own branch (`slug(key + title)`).
    if (ticket) return null;
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
    // A ticket start stays on its own new branch until someone picks a row.
    if (current === null && untrack(() => ticket) && untrack(() => newWorktreeName)) return;
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

  // ── Work key (roadmap M1) ──
  // The ticket / workstream key this session will carry, read from the
  // branch it will run on: the new branch (which follows the Name field, so
  // "ABC-123 Fix login" gives `abc-123-fix-login`) or the picked worktree's.
  // The sidebar groups by it (work_keys.ts). When a live session already
  // carries the same key, say so and offer to open it instead of starting a
  // duplicate — never block: a second session on a ticket is legitimate.
  const plannedKey = $derived(
    inNewMode
      ? extractWorkKey(newWorktreeName)
      : extractWorkKey(chosenWorktree?.branch ?? chosenWorktree?.name ?? null),
  );
  const branchById = $derived(worktreeBranchById($projects));
  const duplicateOf = $derived(
    plannedKey
      ? ($sessions.find(
          (s) =>
            s.status !== 'ghost' &&
            s.kind !== 'external' &&
            workKeyFor(s, branchById)?.key === plannedKey,
        ) ?? null)
      : null,
  );
  // A key with only PAST work (M2.5): say so and offer to resume it rather
  // than start from nothing. Read from the hub per key; an older hub has no
  // answer and the note stays the plain one.
  let pastOfKey = $state<{ key: string; links: WorkLink[] } | null>(null);
  let resumeOpen = $state(false);
  $effect(() => {
    const key = plannedKey;
    if (!key || duplicateOf) return;
    void endedWorkLinks(key).then((r) => {
      if (plannedKey !== key) return;
      pastOfKey = r.ok && Array.isArray(r.value) && r.value.length > 0 ? { key, links: r.value } : null;
    });
  });
  const pastWork = $derived(
    pastOfKey && pastOfKey.key === plannedKey && !duplicateOf ? pastOfKey.links : null,
  );
  function openDuplicate() {
    if (!duplicateOf) return;
    selectSessionExplicitly(duplicateOf);
    onCancel();
  }
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
  // new_session routes, so it only needs the live connection to be up.
  const newSessionBlocked = $derived(hubActionBlocked('new_session', $hubStatus, $hubConnection));
  const projectsLayout = $derived(settingLayout($fleetSettings));
  const remoteRoot = $derived(
    settingPathMap($fleetSettings, PROJECTS_RESOLVED_KEY)[chosenHost] ?? projectsDefaultRoot(projectsLayout),
  );
  onMount(() => {
    // The remote preview needs the backend's per-host roots; best effort.
    void loadFleetSettings();
    // "The New-session dialog opening" is a usage fetch trigger. The backend
    // keeps the 5-minute floor; a refused or failed refresh just leaves the
    // last-known snapshot, so nothing is surfaced here. `refresh_account_usage`
    // is local-only in remote mode (same as HostsView's), so skip it there.
    if (ownsTheFleet($hubStatus)) {
      const uuids = new Set(
        $hosts.filter((h) => !h.hidden && h.account_uuid).map((h) => h.account_uuid as string),
      );
      for (const uuid of uuids) void refreshAccountUsage(uuid);
    }
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
    if (hostWorktrees.status === 'unlistable') return null;
    if (!hostWorktrees.cloned) return `Not cloned on ${chosenHost} yet — it is cloned on the first session.`;
    return null;
  });
  // Non-null when the hub could not answer the scan at all (`HUB_CANNOT_SCAN`
  // above), so the picker only offers "+ new worktree" for that host. A
  // neutral note, not an error — deliberately separate from
  // `worktreeStatus`'s scanning / error / not-cloned states, which are about
  // the host rather than about the hub.
  const remoteWorktreesUnlistable = $derived(
    hostWorktrees.status === 'unlistable'
      ? `Existing worktrees on ${chosenHost} can't be listed through this hub — create a new one, or start from the project root.`
      : null,
  );

  let busy = $state(false);
  let error: string | null = $state(null);
  let createController: AbortController | null = null;

  // Escape / backdrop close this dialog during a creation in both modes
  // (`Modal.svelte` never gates that on `busy`) without aborting the
  // request — a late SUCCESS still merges into the session store on its own
  // (`newSessionAbortable` → `acceptCommandRow`), but a late FAILURE has
  // nowhere left to show `error`: the paragraph that would render it is
  // gone with the dialog. Track that so `submit()` can fall back to a toast.
  let destroyed = false;
  onDestroy(() => {
    destroyed = true;
  });

  // A hub-routed `new_session` sends no `call_id` (`HubBackend::new_session`,
  // deliberately — it has no hub-side counterpart to cancel by), so
  // aborting the wait here does not stop the hub from finishing the create:
  // the session it was building appears anyway, right after "Cancel
  // creation" made it look gone. Parity or refusal — and there is no hub
  // tool to route a cancel to — so this dialog refuses instead: while a
  // hub-client creation is in flight, say so rather than offer a button that
  // would only abandon the local wait.
  const hubCreateNote = $derived(
    busy && $hubStatus.remote
      ? `${$hubStatus.url ?? 'the hub'} is creating this session — it can't be cancelled from here, and will appear when it's ready.`
      : null,
  );

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
    // A pasted ticket URL becomes its key, ready for a title to follow.
    const fromUrl = keyFromTicketUrl(value);
    if (fromUrl) value = `${fromUrl} `;
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

  // ── Ticket start (work graph M3) ──
  // With a ticket, creating is `start_work`: the same session, linked
  // `started`, and — when "Brief Claude" is on — the ticket's context queued
  // for the first hook (the description fenced as untrusted). The preview is
  // editable; an untouched preview lets the backend build the canonical one.
  let briefOn = $state(true);
  let briefEdited = $state(false);
  let briefDraft = $state('');
  const briefBranch = $derived(
    inNewMode ? finalizeBranchSlug(newWorktreeName) : (chosenWorktree?.name ?? ''),
  );
  const briefPreview = $derived(ticket ? ticketBriefPreview(ticket, briefBranch) : '');
  const briefText = $derived(briefEdited ? briefDraft : briefPreview);
  function onBriefInput(v: string) {
    briefDraft = v;
    briefEdited = true;
  }
  const startBlocked = $derived(hubActionBlocked('start_work', $hubStatus, $hubConnection));

  // ── Multi-repo start (work graph M9.6) ──
  // One ticket, one sibling session per repository, all on the same branch
  // name (D11). Offered: the projects this key ran in before.
  let ticketPast = $state<WorkLink[]>([]);
  $effect(() => {
    const key = ticket?.key;
    if (!key) return;
    void endedWorkLinks(key).then((r) => {
      if (ticket?.key === key && r.ok && Array.isArray(r.value)) ticketPast = r.value;
    });
  });
  const siblings = $derived(
    ticket?.key ? siblingCandidates(ticket.key, projectId, ticketPast, $sessions, $projects) : [],
  );
  let alsoIn = $state<number[]>([]);
  function toggleAlso(id: number, on: boolean) {
    alsoIn = on ? [...alsoIn.filter((x) => x !== id), id] : alsoIn.filter((x) => x !== id);
  }
  const multiBlocked = $derived(hubActionBlocked('start_work_multi', $hubStatus, $hubConnection));

  async function submitMulti(t: TicketRow, host: string) {
    if (multiBlocked) return;
    busy = true;
    error = null;
    const r = await startWorkMulti({
      ...(t.id != null && t.tracker_id != null ? { item_id: t.id } : { reference: t.key ?? '' }),
      project_ids: [projectId, ...alsoIn],
      host_alias: host,
      name: friendlyName.trim() || undefined,
      worktree: inNewMode ? newWorktreeName.trim() : (chosenWorktree?.name ?? undefined),
      with_brief: briefOn,
      // The brief and name as edited, the same as a single start: the
      // backend applies them to every sibling.
      brief: briefOn && briefEdited ? briefDraft : undefined,
    });
    busy = false;
    if (!r.ok) {
      if (destroyed) pushError(r.error, 'Start work failed');
      else error = r.error.message;
      return;
    }
    const labelOf = (id: number) => {
      const p = $projects.find((x) => x.project.id === id)?.project;
      return p ? `${p.owner}/${p.repo}` : `project ${id}`;
    };
    const note = multiStartNote(r.value, labelOf);
    const first = r.value.started[0];
    if (!first) {
      error = note ?? 'Nothing was started';
      return;
    }
    if (note) push({ kind: 'info', message: note });
    onCreate(first);
  }

  async function submitTicket(t: TicketRow) {
    if (startBlocked) return;
    if (inNewMode) {
      const cleaned = finalizeBranchSlug(newWorktreeName);
      if (cleaned !== newWorktreeName) newWorktreeName = cleaned;
      if (!newWorktreeName.trim()) {
        error = 'Worktree name required';
        return;
      }
    }
    const submittedHost = chosenHost;
    const submittedWorktreeId = inNewMode ? null : chosenWorktreeId;
    if (alsoIn.length > 0) {
      await submitMulti(t, submittedHost);
      if (!error) remember(submittedHost, submittedWorktreeId);
      return;
    }
    busy = true;
    error = null;
    const r = await startWork({
      ...(t.id != null && t.tracker_id != null ? { item_id: t.id } : { reference: t.key ?? '' }),
      project_id: project.project.id,
      host_alias: submittedHost,
      name: friendlyName.trim() || undefined,
      worktree: inNewMode ? newWorktreeName.trim() : (chosenWorktree?.name ?? undefined),
      with_brief: briefOn,
      brief: briefOn && briefEdited ? briefDraft : undefined,
    });
    busy = false;
    if (!r.ok) {
      const d = r.error.details as { session_id?: number } | null | undefined;
      if (r.error.code === 'E_EXISTS' && d?.session_id != null) {
        const live = $sessions.find((x) => x.id === d.session_id);
        if (live) {
          selectSessionExplicitly(live);
          onCancel();
          return;
        }
      }
      if (destroyed) pushError(r.error, 'Start work failed');
      else error = r.error.message;
      return;
    }
    remember(submittedHost, submittedWorktreeId);
    onCreate(r.value);
  }

  async function submit() {
    if (busy) return;
    if (ticket && chosenKind === 'work') {
      await submitTicket(ticket);
      return;
    }
    // The Create button's `disabled` reads the same derived, but Enter in
    // any field (`onKeydown` below) calls `submit()` directly — the handler
    // must refuse too, or a blocked hub client could still route
    // `new_session` from the keyboard.
    if (newSessionBlocked) return;
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
      // `E_CANCELLED` is the user's own "Cancel creation" (local mode only
      // — a hub-routed create offers none, see `hubCreateNote`); never a
      // failure worth reporting.
      if (r.error.code !== 'E_CANCELLED') {
        // While the dialog is still open, its own paragraph shows this. Once
        // it's gone (Escape / backdrop closed it mid-create), that paragraph
        // closed with it — a toast is the only way this failure still
        // reaches anyone, in both modes.
        if (destroyed) {
          pushError(r.error, 'New session failed');
        } else {
          error = r.error.message;
        }
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

<Modal label="New session" onclose={onCancel} width="520px">
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
    {#if plannedKey}
      <p class="work-note" data-testid="work-note">
        <span class="k">work</span> <code>{plannedKey}</code>
        {#if duplicateOf}
          <span class="dup" data-testid="work-duplicate">
            — already running as <b>{duplicateOf.friendly_name ?? duplicateOf.tmux_name}</b>
            on {duplicateOf.host_alias}.
            <button type="button" class="linkish" data-testid="open-duplicate" onclick={openDuplicate}
              >Open it</button
            >
          </span>
        {:else if pastWork}
          <span class="dup" data-testid="work-past">
            — has previous work ({pastWorkSummary(pastWork, now * 1000)}).
            <button type="button" class="linkish" data-testid="resume-past" onclick={() => (resumeOpen = true)}
              >Resume</button
            >
          </span>
        {:else}
          <span class="muted">— sessions on this branch group under it (sidebar: by work)</span>
        {/if}
      </p>
    {/if}
    {#if ticket}
      <div class="ticket-box" data-testid="ticket-box">
        <p class="work-note">
          <span class="k">ticket</span> <code>{ticket.key}</code>
          {ticket.title}{#if ticket.status_name}<span class="muted"> — {ticket.status_name}</span>{/if}
        </p>
        {#if chosenKind === 'work'}
          <label class="brief-toggle">
            <input type="checkbox" data-testid="ticket-brief-on" bind:checked={briefOn} />
            Brief Claude with the ticket
          </label>
          {#if briefOn}
            <textarea
              class="brief"
              data-testid="ticket-brief"
              rows="6"
              value={briefText}
              oninput={(e) => onBriefInput((e.target as HTMLTextAreaElement).value)}
            ></textarea>
            <p class="muted small">
              Delivered with the first prompt (never typed into the pane). The description is
              the ticket author's text and stays fenced as untrusted.
            </p>
          {/if}
          {#if siblings.length > 0}
            <fieldset class="also-in" data-testid="ticket-also-in">
              <legend>Also start in <span class="muted small">(one session each, same branch)</span></legend>
              {#each siblings as c (c.id)}
                <label>
                  <input
                    type="checkbox"
                    data-testid="ticket-also-in-{c.id}"
                    checked={alsoIn.includes(c.id)}
                    onchange={(e) => toggleAlso(c.id, (e.target as HTMLInputElement).checked)}
                  />
                  {c.label}
                </label>
              {/each}
            </fieldset>
          {/if}
        {/if}
      </div>
    {/if}
    {#if resumeOpen && plannedKey}
      <ResumeDialog workKey={plannedKey} onclose={() => (resumeOpen = false)} onresumed={onCancel} />
    {/if}

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

    <HostChips
      active={chosenHost}
      labelId="new-session-host-label"
      showUsage
      {now}
      {locale}
      {timeZone}
      onpick={(alias) => {
        chosenHost = alias;
        nameOverride = null;
      }}
    />

    <label for="wt-picker">Worktree</label>
    {#if worktreeStatus}
      <p class="wt-status" data-testid="wt-status" class:err={hostWorktrees.status === 'error'}>{worktreeStatus}</p>
    {/if}
    {#if remoteWorktreesUnlistable}
      <p class="wt-status" data-testid="wt-remote-unknown">{remoteWorktreesUnlistable}</p>
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
    {#if hubCreateNote}
      <span class="hub-create-note" data-testid="hub-create-note" title={hubCreateNote}>{hubCreateNote}</span>
    {:else if busy}
      <button type="button" data-testid="cancel-create" onclick={cancelCreate}>Cancel creation</button>
    {:else}
      <button
        class="primary"
        onclick={submit}
        data-testid="create-btn"
        disabled={(inNewMode && !newWorktreeName.trim()) ||
          (ticket && chosenKind === 'work' ? startBlocked !== null : newSessionBlocked !== null)}
        title={(ticket && chosenKind === 'work' ? startBlocked : newSessionBlocked) ?? ''}
      >{ticket && chosenKind === 'work' ? 'Start work' : 'Create'}</button>
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
  .work-note {
    margin: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .work-note .dup { color: var(--fg); }
  .work-note .linkish {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: inherit;
    text-decoration: underline;
  }
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
  .hub-create-note { font-size: 0.68rem; color: var(--fg-muted); text-align: right; }
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
  .ticket-box {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .brief-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.8rem;
  }
  .brief {
    font: inherit;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 0.72rem;
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
  }
  .small {
    font-size: 0.7rem;
  }
</style>
