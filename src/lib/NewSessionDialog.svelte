<script lang="ts">
  import Icon from './kit/Icon.svelte';
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import { listHostWorktrees, projects, type ProjectTreeRow, type WorktreeRow } from './projects';
  import { extractWorkKey, keyFromTicketUrl, workKeyFor, worktreeBranchById } from './work_keys';
  import { endedWorkLinks, pastWorkSummary, type WorkLink } from './work';
  import ResumeDialog from './ResumeDialog.svelte';
  import { selectSessionExplicitly } from './selection';
  import { newBgSession, newSessionAbortable, sessions, type SessionRow } from './sessions';
  import { defaultHost, hosts, isPickableHost } from './hosts';
  import { readPref, writePref } from './prefs';
  import { MODEL_OPTIONS, LAUNCH_EFFORT_OPTIONS, CODEX_MODEL_OPTIONS, CODEX_EFFORT_OPTIONS } from './conversation';
  import { slugifyBranch, finalizeBranchSlug } from './branch-slug';
  import { generateName, nameWords, tmuxNameSuffix } from './names';
  import Modal from './Modal.svelte';
  import PickerList from './PickerList.svelte';
  import HostChips from './HostChips.svelte';
  import { refreshAccountUsage } from './account_usage_store';
  import { accountByUuid, accountLabel } from './accounts';
  import {
    checkAccountHeadroom,
    freestLogin,
    loginLabel,
    usedText,
    type Headroom,
    type HostLogin,
  } from './account_limits';
  import { push, pushError } from './toasts';
  import { errorDetail, errorSentence, errorText } from './error_copy';
  import type { IpcError } from './result';
  import type { PickerItem } from './PickerList.svelte';
  import {
    fleetSettings,
    loadFleetSettings,
    settingBool,
    SETTING_KEYS,
    settingPathMap,
    settingLayout,
    projectDir,
    projectsDefaultRoot,
    PROJECTS_RESOLVED_KEY,
  } from './fleet_settings';
  import { hubStatus, ownsTheFleet, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { startWork, ticketBriefPreview, ticketRef, type TicketRow } from './trackers';
  import { startWorkMulti, siblingCandidates, multiStartNote, multiStartToast, shownSiblings } from './multi_start';
  import ProposedBy from './ProposedBy.svelte';
  import StartPulse from './StartPulse.svelte';
  import { creatingStart } from './session_starting';
  import { correctionLine, preselect, type ProposalLike } from './ai_proposal';
  import { HOST_PLACEMENT_FLOOR, hostProposal, proposeHostPlacement, recordHostPlacement } from './host_placement';
  import {
    RESUME_OR_NEW_FLOOR,
    START_FRESH,
    followResumeOrNew,
    proposeResumeOrNew,
    resumeChoice,
    resumeProposal,
    type ResumeOrNew,
  } from './resume_or_new';
  import DraftField from './DraftField.svelte';
  import {
    draftBrief,
    draftSource,
    previewStartWork,
    siblingProposal,
    type BriefDraft,
    type StartConflict,
  } from './start_preview';
  import { newStartToken, NO_START_STEPS } from './start_steps';
  import { submitHint, submitKey } from './forms/form_frame';
  import { detectMac } from './terminal_keys';
  import { openNewSessionPicker } from './switcher_request';
  import { radiogroupKeys } from './radiogroup_keys';
  import { agentChoices, keepAgent, type PickerAgent } from './agent_picker';
  import {
    PRIMARY,
    fleets,
    fleetRows,
    fleetChoicesFor,
    fleetCloneProject,
    fleetHostChoices,
    defaultFleetHost,
    defaultFleetFor,
    rememberedFleetFor,
    rememberFleetFor,
    fleetNewSession,
    openFleetSession,
    multiFleet,
  } from './fleets';
  import { fleetProfiles, loadProfile, profileLines } from './fleet_profiles';

  let {
    project,
    onCreate,
    onCancel,
    initialName,
    initialHost,
    initialKind,
    ticket,
    autostart = false,
    proposal = null,
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
    /** Preselect Work or Shell (the Hosts view's "Open a shell", G4.5);
     *  wins over the project's remembered kind. */
    initialKind?: 'work' | 'shell';
    /** Start work on this ticket (work graph M3): the dialog offers "Brief
     *  Claude with the ticket" with an editable preview, and creating goes
     *  through `start_work`, which links the session `started`. */
    ticket?: TicketRow;
    /** Start at once with the remembered choices (the picker's ⌘↵); the
     *  dialog stays open only if something needs a person. */
    autostart?: boolean;
    /** What chose `project` (redesign 3.12, K1): shown as the shared chip
     *  in the dialog; Change re-opens the picker for another one. */
    proposal?: ProposalLike | null;
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
  // last-host, then `defaultHost` (`local`, or the first pickable host on a
  // hub without one). Mirrors the chip `disabled` rule in HostChips.
  function usableHost(alias: string | null | undefined): alias is string {
    return isPickableHost($hosts, alias);
  }
  let chosenHost = $state<string>(
    untrack(
      () =>
        [initialHost, memory?.host, readPref('last-host', '', isString)].find(usableHost) ??
        defaultHost($hosts),
    ),
  );
  $effect(() => {
    // Jev's proposed host is not a pick: it is remembered once the person
    // starts on it (`answerHostProposal`), not merely for being shown.
    if (hostProposed?.value === chosenHost) return;
    writePref('last-host', chosenHost);
  });

  // Jev N5 host placement (redesign step 4.11): with no host
  // the project's rule keeps (the one the dialog was opened on, or the one
  // remembered for this project), the decision model may propose one of the
  // online hosts under their limit. Off by default; a hub client is refused
  // and keeps today's default. A person's pick always wins, and the proposal
  // only ever lands while nobody has picked yet.
  const hostByRule = untrack(() => [initialHost, memory?.host].some(usableHost));
  let hostPicked = false;
  let hostProposed = $state<ProposalLike | null>(null);
  // Whether a proposal was shown: the start then answers it.
  let hostProposalShown = false;
  let hostBeforeProposal: string | null = null;
  onMount(() => {
    if (hostByRule || autostart) return;
    void proposeHostPlacement(project.project.id).then((r) => {
      if (destroyed || hostPicked || !r.ok || !r.value) return;
      const p = hostProposal(r.value);
      const alias = preselect('host', p, HOST_PLACEMENT_FLOOR);
      if (!alias || !usableHost(alias)) return;
      hostBeforeProposal = chosenHost;
      chosenHost = alias;
      hostProposed = p;
      hostProposalShown = true;
    });
  });
  // The corrected state (G4.9): the person moved off Jev's host. The start
  // records it (`answerHostProposal`), so the line says so.
  let hostCorrection = $state<{ from: string; to: string } | null>(null);
  function correctHost(to: string) {
    const from = hostProposed?.value ?? hostCorrection?.from ?? null;
    hostCorrection = hostProposalShown && from && from !== to ? { from, to } : null;
  }
  function pickHost(alias: string) {
    correctHost(alias);
    hostPicked = true;
    hostProposed = null;
    chosenHost = alias;
  }
  function changeProposedHost() {
    if (hostBeforeProposal) chosenHost = hostBeforeProposal;
    correctHost(chosenHost);
    hostPicked = true;
    hostProposed = null;
  }
  /** Jev N5's follow-up: the host a person started on answers the proposal
   *  they were shown (best effort; local-only, off by default). */
  function answerHostProposal(host: string) {
    writePref('last-host', host);
    if (hostProposalShown) void recordHostPlacement(project.project.id, host);
    // Starting a session here is starting fresh: the N2 proposal's answer.
    answerResumeProposal(START_FRESH);
  }

  // "work" runs Claude Code in the pane; "shell" runs a plain login shell.
  let chosenKind = $state<'work' | 'shell'>(untrack(() => initialKind ?? memory?.kind ?? 'work'));
  // Whether the person picked the kind here. A kind the opener preset
  // (`initialKind`, the Hosts view's "Open a shell") is for this open only and
  // is not remembered as the project's (review w12-12).
  let kindPicked = false;
  // Which agent a "work" session runs (redesign 12.4): Claude Code, or an
  // agent the chosen host has on its PATH. A ticket's start and a
  // background run are Claude Code's own paths, so they keep it.
  let chosenAgent = $state<PickerAgent>('claude');
  const agentOptions = $derived(
    hostAgentChoices().map((c) =>
      c.agent !== 'claude' && c.enabled && ticket
        ? { ...c, enabled: false, reason: `A ticket starts with Claude Code`, tag: null }
        : c,
    ),
  );
  /** The agents the host the session starts on has: on an extra fleet, its
   *  host's (review w01-2). A function so it may read the fleet picker's
   *  state, declared further down. */
  function hostAgentChoices() {
    return fleetChoice
      ? agentChoices(fleetHost, fleetHosts.find((h) => h.alias === fleetHost))
      : agentChoices(chosenHost, $hosts.find((h) => h.alias === chosenHost));
  }
  $effect(() => {
    const next = keepAgent(untrack(() => chosenAgent), agentOptions);
    untrack(() => {
      if (next !== chosenAgent) chosenAgent = next;
    });
  });
  /** The session runs Claude Code: its model, effort, profile, limit check,
   *  background run and ticket brief apply. */
  const runsClaude = $derived(chosenKind === 'work' && chosenAgent === 'claude');
  // Optional command run on start for a shell session (empty = bare shell).
  let startCommand = $state<string>('');
  // `claude --model` / `--effort` for a Claude session; '' = the host's
  // default. The last choice is remembered across projects.
  const isLaunchModel = (v: unknown): v is string => typeof v === 'string' && (v === '' || MODEL_OPTIONS.some((o) => o.value === v));
  const isLaunchEffort = (v: unknown): v is string =>
    typeof v === 'string' && (v === '' || LAUNCH_EFFORT_OPTIONS.some((o) => o.value === v));
  let chosenModel = $state<string>(readPref('newsession.model', '', isLaunchModel));
  let chosenEffort = $state<string>(readPref('newsession.effort', '', isLaunchEffort));
  $effect(() => {
    writePref('newsession.model', chosenModel);
    writePref('newsession.effort', chosenEffort);
  });
  /** The session runs Codex: `codex -m` / `model_reasoning_effort` take
   *  Codex's own lists, remembered apart from Claude's. */
  const runsCodex = $derived(chosenKind === 'work' && chosenAgent === 'codex');
  const isCodexModel = (v: unknown): v is string =>
    typeof v === 'string' && (v === '' || CODEX_MODEL_OPTIONS.some((o) => o.value === v));
  const isCodexEffort = (v: unknown): v is string =>
    typeof v === 'string' && (v === '' || CODEX_EFFORT_OPTIONS.some((o) => o.value === v));
  let codexModel = $state<string>(readPref('newsession.codex.model', '', isCodexModel));
  let codexEffort = $state<string>(readPref('newsession.codex.effort', '', isCodexEffort));
  $effect(() => {
    writePref('newsession.codex.model', codexModel);
    writePref('newsession.codex.effort', codexEffort);
  });
  // Credential profile (`~/.claude-profiles/<name>` on the host, its own
  // `/login`; docs/accounts.md); '' = the host's login. Deliberately not
  // remembered: which account a session bills is chosen each time.
  let chosenProfile = $state<string>('');
  // The chosen host's known profiles, offered as suggestions; a new name
  // is still accepted (the session asks for its /login).
  const hostProfiles = $derived($hosts.find((h) => h.alias === chosenHost)?.claude_profiles ?? []);
  // Limit handling (redesign step 4.4): a start on an account past
  // `accounts.pause_at` asks first and offers the login with headroom.
  // Cleared when the host or the profile changes, so an answer never
  // carries over to a different login.
  let limitAsk = $state<Headroom | null>(null);
  let limitConfirmed = $state(false);
  $effect(() => {
    void chosenHost;
    void chosenProfile;
    untrack(() => {
      limitAsk = null;
      limitConfirmed = false;
    });
  });
  const accountName = (uuid: string) => accountLabel($accountByUuid.get(uuid));
  function useLogin(l: HostLogin) {
    chosenProfile = l.profile ?? '';
    // The effect above clears the flags on the profile change; confirm after it.
    void tick().then(() => {
      limitConfirmed = true;
      void submit();
    });
  }
  function startAnyway() {
    limitConfirmed = true;
    limitAsk = null;
    void submit();
  }
  // Redesign step 4.5: the login is picked from the host's
  // logins with their live usage, defaulting to the one with the most
  // headroom until the person picks; "Other profile…" brings back the free
  // name field (a new profile still asks for its /login in the pane).
  const OTHER_PROFILE = '\u0000other';
  let hostLogins = $state<HostLogin[] | null>(null);
  let pickedLogin = false;
  let otherProfile = $state(false);
  let loginsHost: string | null = null;
  $effect(() => {
    const host = chosenHost;
    untrack(() => {
      hostLogins = null;
      pickedLogin = false;
      otherProfile = false;
      // A login belongs to its host: a host change drops the last one, so
      // Create never sends a profile the select no longer shows (review r05
      // A4). The first host keeps a prefilled profile.
      if (loginsHost !== null && loginsHost !== host) chosenProfile = '';
      loginsHost = host;
    });
    void checkAccountHeadroom(host, null).then((h) => {
      if (chosenHost !== host) return;
      const logins = h.ok && Array.isArray(h.value?.logins) ? h.value.logins : [];
      hostLogins = logins;
      const best = pickedLogin ? null : freestLogin(logins);
      if (best) chosenProfile = best.profile ?? '';
    });
  });
  // A name typed while the headroom check is still out is a pick too: the
  // check's default must not overwrite it, and the field stays (review r05).
  function onTypeProfile() {
    pickedLogin = true;
    otherProfile = true;
  }
  // The picked login's account, so the host line and warning describe the
  // account the session will run on (review r05 A8); null keeps the host's.
  const chosenLoginAccount = $derived(hostLogins?.find((l) => (l.profile ?? '') === chosenProfile)?.account_uuid ?? null);
  function onPickLogin(v: string) {
    pickedLogin = true;
    if (v === OTHER_PROFILE) {
      otherProfile = true;
      chosenProfile = '';
    } else {
      chosenProfile = v;
    }
  }
  // "Run: in background" (step 4.5): a supervised background session, what
  // the sidebar's ⚡ dialog launches, which stays as it was.
  let runBackground = $state(false);
  let bgPrompt = $state('');
  const bgSessionBlocked = $derived(hubActionBlocked('new_bg_session', $hubStatus, $hubConnection));
  const asBackground = $derived(runBackground && runsClaude && !ticket);
  async function submitBackground() {
    if (bgSessionBlocked !== null) return;
    if (!bgPrompt.trim()) {
      error = 'A background session needs its first prompt';
      return;
    }
    busy = true;
    error = null;
    const host = chosenHost;
    const bgName = friendlyName.trim() || nameWords(generateName(takenSlugs));
    // In this project's checkout on the host, not its home folder (review
    // w12-9): the dialog is "New session — owner/repo".
    const r = await newBgSession(host, bgName, bgPrompt.trim(), null, projectId);
    busy = false;
    if (!r.ok) {
      if (destroyed) pushError(r.error, 'Background session failed');
      else error = r.error;
      return;
    }
    // "Over pause_at", "no session id, limits not recorded" (review w12-10).
    if (r.value?.warning) push({ kind: 'warning', message: r.value.warning });
    const row = r.value?.session ?? null;
    if (row) {
      finished(row, host);
      return;
    }
    push({ kind: 'info', message: `Started ${bgName} in the background on ${host}` });
    closeSelf();
  }
  const profileInvalid = $derived(
    chosenProfile.trim() !== '' && !/^[A-Za-z0-9][A-Za-z0-9_-]{0,31}$/.test(chosenProfile.trim()),
  );

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
    // Review r13 D19: the code and the backend's words, shown as one plain
    // sentence with the original under Details.
    error?: Pick<IpcError, 'code' | 'message'>;
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
  // "Scan again" bumps this; the scan effect reads it, so it re-runs.
  let rescans = $state(0);
  $effect(() => {
    const host = chosenHost;
    void rescans;
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
          : { status: 'error', rows: [], cloned: true, error: r.error };
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
          error: { code: 'E_PARSE', message: `the worktree scan answered for another host than ${host}` },
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
  // Gap plan G7.10 (Jev N2): which to do with that past work — resume one
  // past session or start fresh. A rule answers for one recent past
  // session; Jev only at assist (off by default). Asked once the key's past
  // work is known; with no proposal the notice stays the plain one.
  let resumeAsk = $state<{ key: string; p: ResumeOrNew } | null>(null);
  /** The key whose proposal the person set aside ("Start fresh instead"). */
  let resumeDismissed = $state<string | null>(null);
  /** The past session the resume flow opens on (the proposal's), or none. */
  let resumeLink = $state<{ link: number; session: number | null } | null>(null);
  $effect(() => {
    const key = plannedKey;
    if (!key || duplicateOf) return;
    void endedWorkLinks(key).then((r) => {
      if (plannedKey !== key) return;
      pastOfKey = r.ok && Array.isArray(r.value) && r.value.length > 0 ? { key, links: r.value } : null;
      if (!pastOfKey || resumeAsk?.key === key) return;
      void proposeResumeOrNew(key).then((p) => {
        if (plannedKey === key && p?.ok && p.value) resumeAsk = { key, p: p.value };
      });
    });
  });
  const pastWork = $derived(
    pastOfKey && pastOfKey.key === plannedKey && !duplicateOf ? pastOfKey.links : null,
  );
  /** The proposal the notice shows, as ProposedBy reads it; null below the
   *  floor, on unsure, once set aside, or for another key. */
  const resumeShown = $derived.by(() => {
    if (!pastWork || !resumeAsk || resumeAsk.key !== plannedKey || resumeDismissed === plannedKey) return null;
    const p = resumeProposal(resumeAsk.p);
    return preselect('resume', p, RESUME_OR_NEW_FLOOR) != null ? p : null;
  });
  const resumeShownName = $derived(resumeShown ? (resumeAsk?.p.name ?? plannedKey) : null);
  /** Tell the hub what the person picked over a Jev proposal they saw (a
   *  rule's has nothing to mark). Best effort. */
  function answerResumeProposal(chosen: string) {
    if (!plannedKey || resumeShown?.source !== 'jev') return;
    void followResumeOrNew(plannedKey, chosen);
  }
  function openResume(proposed: boolean) {
    const p = resumeAsk?.p;
    resumeLink = proposed && p?.link_id != null ? { link: p.link_id, session: p.session_id ?? null } : null;
    resumeOpen = true;
  }
  function startFreshInstead() {
    answerResumeProposal(START_FRESH);
    resumeDismissed = plannedKey;
  }
  function onResumed(link: number | null) {
    if (link != null) answerResumeProposal(resumeChoice(link));
    onCancel();
  }
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
      // The profiles' accounts too: the Account select shows their usage.
      for (const h of $hosts) {
        if (h.hidden) continue;
        for (const p of h.claude_profiles ?? []) if (p.account_uuid) uuids.add(p.account_uuid);
      }
      for (const uuid of uuids) void refreshAccountUsage(uuid);
    }
  });
  const pathPreview = $derived.by(() => {
    const root =
      chosenHost === 'local' ? project.project.base_path : projectDir(remoteRoot, projectsLayout, owner, repo);
    if (inNewMode) {
      const slug = finalizeBranchSlug(newWorktreeName);
      // The branch keeps its `/`; the directory is flat (`feat/x` →
      // `feat-x`), as `projects::worktree_dir_name` creates it.
      return slug ? `${root}/${worktreeDir}/${slug.replaceAll('/', '-')}` : root;
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
    // The error state has its own block (Details, Scan again) below.
    if (hostWorktrees.status === 'error') return null;
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
  // A string is the dialog's own sentence; an `IpcError` is shown as one
  // plain sentence with the backend's words under Details (review w12-6).
  let error: string | IpcError | null = $state(null);
  // Only a plain create can be cancelled: "Cancel creation" shows while this
  // is set, never for a ticket, multi-repo, background or fleet start, nor
  // during the limit check (review w12-3).
  let createController = $state<AbortController | null>(null);
  let errorEl: HTMLDivElement | undefined = $state();
  $effect(() => {
    // The failure sits under the fields, which scroll; bring it into view.
    if (error) errorEl?.scrollIntoView?.({ block: 'nearest' });
  });

  /** A start that finished: open it. When the dialog was closed (Escape)
   *  while it ran, say so in a toast instead, so a later New session dialog
   *  is never closed and the selection never jumps under the person
   *  (review w12-5). */
  function finished(row: SessionRow, host: string) {
    if (destroyed) {
      push({
        kind: 'success',
        message: `Started ${row.friendly_name || row.tmux_name} on ${host}`,
        action: { label: 'Open', run: () => selectSessionExplicitly(row) },
      });
      return;
    }
    onCreate(row);
  }
  /** Close this dialog, unless it is already gone (review w12-5). */
  function closeSelf() {
    if (!destroyed) onCancel();
  }

  /** A ticket start in flight shows the start Pulse too (review w12-4).
   *  `start_work` reports no steps, so it shows the first one running. */
  async function withStartPulse<T>(host: string, run: () => Promise<T>): Promise<T> {
    creatingStart.set({ host_alias: host, name: friendlyName.trim(), kind: 'work', token: newStartToken(), steps: NO_START_STEPS });
    try {
      return await run();
    } finally {
      creatingStart.set(null);
    }
  }

  // ── Which fleet (multi-hub Phase 2) ──────────────────────────────────
  // The extra fleets that also have this repository. Picking one starts the
  // session on THAT hub (`fleetNewSession`), with its hosts and its project
  // id; the rest of this dialog (accounts, worktree scan, tickets, multi
  // start) is about this window's own fleet and steps aside.
  const fleetOptions = $derived(fleetChoicesFor(owner, repo, $fleets, $fleetRows));
  let chosenFleet = $state<string>(PRIMARY);
  let fleetPicked = false;
  $effect(() => {
    const options = fleetOptions;
    untrack(() => {
      if (!fleetPicked) chosenFleet = defaultFleetFor(owner, options, rememberedFleetFor(owner, repo));
    });
  });
  const fleetChoice = $derived(
    ticket || asBackground ? null : (fleetOptions.find((c) => c.fleet.id === chosenFleet) ?? null),
  );
  const chosenFleetRows = $derived(fleetChoice ? $fleetRows.get(fleetChoice.fleet.id) : undefined);
  const fleetHosts = $derived(chosenFleetRows ? fleetHostChoices(chosenFleetRows) : []);
  let fleetHost = $state('');
  $effect(() => {
    const list = fleetHosts;
    untrack(() => {
      if (!list.some((h) => h.alias === fleetHost)) fleetHost = chosenFleetRows ? defaultFleetHost(chosenFleetRows) : '';
    });
  });
  let fleetBranch = $state('');

  // Phase 3b: which profile the session runs as — this device's person on
  // the chosen fleet, under the label given here. A read that failed earlier
  // is asked again when the fleet is picked (its line says it failed).
  const runsOnFleet = $derived(fleetChoice ? fleetChoice.fleet.id : PRIMARY);
  const runsAs = $derived($multiFleet ? ($profileLines.get(runsOnFleet)?.line ?? null) : null);
  $effect(() => {
    const id = runsOnFleet;
    if (!$multiFleet) return;
    const st = untrack(() => $fleetProfiles.get(id));
    if (!st || st.error) void loadProfile(id);
  });

  function pickFleet(id: string) {
    fleetPicked = true;
    chosenFleet = id;
  }

  async function submitFleet(choice: NonNullable<typeof fleetChoice>) {
    if (!fleetHost) return;
    busy = true;
    error = null;
    // A fleet without the repository gets it cloned onto the picked host
    // first, so its machines can be used for any repository this window has.
    let projectId = choice.projectId;
    if (projectId === null) {
      const added = await fleetCloneProject(choice.fleet.id, fleetHost, owner, repo);
      if (!added.ok) {
        busy = false;
        if (destroyed) pushError(added.error, `Adding ${owner}/${repo} to ${choice.fleet.label} failed`);
        else error = added.error;
        return;
      }
      projectId = added.value.project.id;
    }
    const r = await fleetNewSession(choice.fleet.id, {
      host_alias: fleetHost,
      project_id: projectId,
      name: nameOverride ?? '',
      friendly_name: friendlyName.trim() || null,
      new_worktree: fleetBranch.trim() ? finalizeBranchSlug(slugifyBranch(fleetBranch.trim())) : null,
      kind: chosenKind,
      // The agent, model and effort picked above go to that hub as they go
      // to this one (review w01-2). The login does not: this window's
      // accounts are its own hub's, so the fleet's host uses its own.
      start_command: chosenKind === 'shell' ? startCommand.trim() || null : null,
      model: runsClaude && chosenModel ? chosenModel : runsCodex && codexModel ? codexModel : null,
      effort: runsClaude && chosenEffort ? chosenEffort : runsCodex && codexEffort ? codexEffort : null,
      agent: chosenKind === 'work' && chosenAgent !== 'claude' ? chosenAgent : null,
    });
    busy = false;
    if (!r.ok) {
      if (destroyed) pushError(r.error, `New session on ${choice.fleet.label} failed`);
      else error = r.error;
      return;
    }
    rememberFleetFor(owner, repo, choice.fleet.id);
    if (destroyed) {
      const id = r.value.id;
      push({
        kind: 'success',
        message: `Started a session on ${choice.fleet.label}`,
        action: { label: 'Open', run: () => openFleetSession(choice.fleet.id, id) },
      });
      return;
    }
    openFleetSession(choice.fleet.id, r.value.id);
    onCancel();
  }

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
    kindPicked = true;
    chosenKind = kind;
    chosenAgent = 'claude';
    nameOverride = null;
  }

  function onPickAgent(agent: PickerAgent) {
    kindPicked = true;
    chosenKind = 'work';
    chosenAgent = agent;
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
      kind: kindPicked || !initialKind ? chosenKind : (prev?.kind ?? 'work'),
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

  // Redesign 6.10: "Draft with Claude" asks the chosen host for a brief
  // written from the ticket and the task's earlier work. The draft is the
  // field's text, the person's to edit; only Start sends it. Clear goes back
  // to the ticket brief.
  let draftMeta = $state<BriefDraft | null>(null);
  let drafting = $state(false);
  let draftError = $state<string | null>(null);
  let draftSeq = 0;
  $effect(() => {
    void ticketKey;
    draftSeq++;
    draftMeta = null;
    drafting = false;
    draftError = null;
  });
  async function draftTicketBrief(t: TicketRow) {
    const mine = ++draftSeq;
    const before = { text: briefDraft, edited: briefEdited };
    briefDraft = briefText;
    briefEdited = true;
    drafting = true;
    draftError = null;
    const r = await draftBrief({
      ...ticketRef(t),
      project_id: project.project.id,
      host_alias: chosenHost,
      worktree: inNewMode ? newWorktreeName.trim() : (chosenWorktree?.name ?? undefined),
    });
    if (mine !== draftSeq) return;
    drafting = false;
    if (!r.ok) {
      // No draft: the field is as it was, the person's edit or the template.
      if (!draftMeta) {
        briefDraft = before.text;
        briefEdited = before.edited;
      }
      draftError = errorText(r.error);
      return;
    }
    briefDraft = r.value.brief;
    draftMeta = r.value.draft;
  }
  function clearDraft() {
    draftSeq++;
    draftMeta = null;
    drafting = false;
    briefEdited = false;
    briefDraft = '';
  }

  // ── Multi-repo start (work graph M9.6) ──
  // One ticket, one sibling session per repository, all on the same branch
  // name (D11). Offered: the projects this key ran in before.
  const ticketKey = $derived(ticket?.key ?? null);
  let ticketPast = $state<WorkLink[]>([]);
  let alsoIn = $state<number[]>([]);
  // A different key (or project) is a different offer: forget the old one's
  // past links and ticked repos, so a stale tick can never start a session
  // in a repository the dialog no longer shows.
  $effect(() => {
    const key = ticketKey;
    void projectId;
    ticketPast = [];
    alsoIn = [];
    if (!key) return;
    void endedWorkLinks(key).then((r) => {
      if (ticketKey === key && r.ok && Array.isArray(r.value)) ticketPast = r.value;
    });
  });
  const siblings = $derived(
    ticketKey ? siblingCandidates(ticketKey, projectId, ticketPast, $sessions, $projects) : [],
  );
  function toggleAlso(id: number, on: boolean) {
    alsoTouched = true;
    alsoIn = on ? [...alsoIn.filter((x) => x !== id), id] : alsoIn.filter((x) => x !== id);
  }

  // Redesign 3.12 (N3): the start preview may carry Jev's
  // proposed sibling; it pre-ticks only while the person has not touched
  // the boxes, and the ProposedBy chip says why. Asked once per ticket,
  // project and host.
  let siblingAsk = $state<ProposalLike | null>(null);
  let alsoTouched = $state(false);
  // The preview's conflicts (review w12-7, w12-8): a start across
  // organisations needs the person's tick, as StartPopover asks; a live
  // session on the key offers "Start another" beside "Open it".
  let startConflicts = $state<StartConflict[]>([]);
  let forceCrossOrg = $state(false);
  /** The refusal's own words when the preview did not see the cross-org rule. */
  let crossOrgRefused = $state<string | null>(null);
  $effect(() => {
    const id = ticket?.id;
    const host = chosenHost;
    const pid = projectId;
    siblingAsk = null;
    alsoTouched = false;
    startConflicts = [];
    untrack(() => {
      forceCrossOrg = false;
      crossOrgRefused = null;
    });
    if (id == null || !host) return;
    void previewStartWork({ item_id: id, project_id: pid, host_alias: host, with_brief: true }).then((r) => {
      // A hub with nothing to preview answers no preview at all; an answer
      // for a ticket, host or project left behind is not this one's.
      if (ticket?.id !== id || chosenHost !== host || projectId !== pid || !r.ok || !r.value) return;
      siblingAsk = siblingProposal(r.value);
      startConflicts = Array.isArray(r.value.conflicts) ? r.value.conflicts : [];
    });
  });
  const crossOrg = $derived(
    ticket ? (startConflicts.find((c) => c.kind === 'cross_org')?.message ?? crossOrgRefused) : null,
  );
  const liveConflict = $derived(ticket ? (startConflicts.find((c) => c.kind === 'live_session') ?? null) : null);
  const liveConflictRow = $derived(
    liveConflict?.session_id != null ? ($sessions.find((x) => x.id === liveConflict.session_id) ?? null) : null,
  );
  /** "Start another": a second session on this ticket, in a checkout of its
   *  own (`parallel`), rather than opening the live one (review w12-8). */
  function startAnother() {
    void submit({ parallel: true });
  }
  function openLiveConflict() {
    if (!liveConflictRow) return;
    selectSessionExplicitly(liveConflictRow);
    onCancel();
  }
  const proposedSibling = $derived.by(() => {
    const v = preselect('sibling', siblingAsk);
    const id = v == null ? null : Number(v);
    return id != null && siblings.some((c) => c.id === id) ? id : null;
  });
  $effect(() => {
    const id = proposedSibling;
    if (id == null || untrack(() => alsoTouched || alsoIn.includes(id))) return;
    alsoIn = [...untrack(() => alsoIn), id];
  });
  const multiBlocked = $derived(hubActionBlocked('start_work_multi', $hubStatus, $hubConnection));

  async function submitMulti(t: TicketRow, host: string, extra: number[], parallel: boolean) {
    if (multiBlocked) return;
    busy = true;
    error = null;
    const args = {
      ...ticketRef(t),
      project_ids: [projectId, ...extra],
      host_alias: host,
      name: friendlyName.trim() || undefined,
      worktree: inNewMode ? newWorktreeName.trim() : (chosenWorktree?.name ?? undefined),
      with_brief: briefOn,
      // The brief and name as edited, the same as a single start: the
      // backend applies them to every sibling.
      brief: briefOn && briefEdited ? briefDraft : undefined,
      ...(forceCrossOrg ? { force_cross_org: true } : {}),
      ...(parallel ? { parallel: true } : {}),
    };
    const r = await withStartPulse(host, () => startWorkMulti(args));
    busy = false;
    if (!r.ok) {
      if (destroyed) pushError(r.error, 'Start work failed');
      else error = r.error;
      return;
    }
    const labelOf = (id: number) => {
      const p = $projects.find((x) => x.project.id === id)?.project;
      return p ? `${p.owner}/${p.repo}` : `project ${id}`;
    };
    const note = multiStartNote(r.value, labelOf);
    const toast = multiStartToast(args, r.value, labelOf);
    const first = r.value.started[0];
    if (!first) {
      error = note ?? 'Nothing was started';
      // "Start anyway" still reaches the repositories only the cross-org
      // rule refused.
      if (toast?.action) push(toast);
      return;
    }
    if (toast) push(toast);
    finished(first, host);
  }

  async function submitTicket(t: TicketRow, parallel = false) {
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
    // Only repositories still offered: never one the dialog does not show.
    const extra = shownSiblings(alsoIn, siblings);
    if (extra.length > 0) {
      await submitMulti(t, submittedHost, extra, parallel);
      if (!error) {
        remember(submittedHost, submittedWorktreeId);
        answerHostProposal(submittedHost);
      }
      return;
    }
    busy = true;
    error = null;
    const args = {
      ...ticketRef(t),
      project_id: project.project.id,
      host_alias: submittedHost,
      name: friendlyName.trim() || undefined,
      worktree: inNewMode ? newWorktreeName.trim() : (chosenWorktree?.name ?? undefined),
      with_brief: briefOn,
      brief: briefOn && briefEdited ? briefDraft : undefined,
      // The person ticked "Start across organisations" (review w12-7).
      ...(forceCrossOrg ? { force_cross_org: true } : {}),
      // "Start another" beside the live session on this key (review w12-8).
      ...(parallel ? { parallel: true } : {}),
    };
    const r = await withStartPulse(submittedHost, () => startWork(args));
    busy = false;
    if (!r.ok) {
      const d = r.error.details as
        | { session_id?: number; orphan_session_id?: number; cross_org?: boolean }
        | null
        | undefined;
      if (r.error.code === 'E_EXISTS' && d?.session_id != null) {
        const live = $sessions.find((x) => x.id === d.session_id);
        if (live) {
          const liveName = live.friendly_name || live.tmux_name;
          if (destroyed) {
            push({
              kind: 'info',
              message: `${t.key} already runs as ${liveName}`,
              action: { label: 'Open', run: () => selectSessionExplicitly(live) },
            });
            return;
          }
          selectSessionExplicitly(live);
          // A lost race: this start's own session spawned and runs unlinked.
          // The hub names it; say so rather than leave it to be found.
          // Otherwise say why the dialog closed into another session rather
          // than switch silently (review w12-8).
          push(
            d.orphan_session_id != null
              ? { kind: 'warning', message: errorText(r.error) }
              : { kind: 'info', message: `${t.key} already runs as ${liveName} — opened it` },
          );
          onCancel();
          return;
        }
      }
      if (d?.cross_org === true && !destroyed) {
        // The refusal tells an API caller to pass `force_cross_org`; a person
        // gets the tick instead (review w12-7).
        crossOrgRefused = errorText(r.error);
        error = 'This ticket belongs to another organisation than this repository. Tick "Start across organisations" to start it anyway.';
        return;
      }
      if (destroyed) pushError(r.error, 'Start work failed');
      else error = r.error;
      return;
    }
    remember(submittedHost, submittedWorktreeId);
    answerHostProposal(submittedHost);
    finished(r.value, submittedHost);
  }

  /** Why the start cannot go yet, or null: the Create button's `disabled`
   *  and title, and the keyboard's guard (review w12-2). */
  const cannotSubmit = $derived.by((): string | null => {
    if (fleetChoice) return fleetHost ? null : `Pick a host on ${fleetChoice.fleet.label}`;
    if (asBackground) {
      return bgSessionBlocked ?? (bgPrompt.trim() ? null : 'A background session needs its first prompt');
    }
    if (inNewMode && !newWorktreeName.trim()) return 'Worktree name required';
    if (chosenKind === 'work' && profileInvalid) {
      return 'A login profile is a name of letters, digits, - and _ (up to 32), such as work';
    }
    if (ticket && chosenKind === 'work') {
      if (startBlocked !== null) return startBlocked;
      if (crossOrg && !forceCrossOrg) return 'Tick "Start across organisations" to start this ticket here';
      return null;
    }
    return newSessionBlocked;
  });

  async function submit(opts: { parallel?: boolean } = {}) {
    if (busy) return;
    // Enter and ⌘↵ read the same predicate the Create button's `disabled`
    // does, so the keyboard cannot send what the button would not (review
    // w12-2: an invalid login profile went to the backend from Enter).
    if (cannotSubmit) {
      error = cannotSubmit;
      return;
    }
    if (fleetChoice) {
      await submitFleet(fleetChoice);
      return;
    }
    if (ticket && chosenKind === 'work') {
      await submitTicket(ticket, opts.parallel === true);
      return;
    }
    // The Create button's `disabled` reads the same derived, but Enter in
    // any field (`onKeydown` below) calls `submit()` directly — the handler
    // must refuse too, or a blocked hub client could still route
    // `new_session` from the keyboard.
    if (asBackground) {
      await submitBackground();
      return;
    }
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
    if (runsClaude && !limitConfirmed) {
      busy = true;
      const h = await checkAccountHeadroom(submittedHost, chosenProfile.trim() || null);
      busy = false;
      // No answer (a hub client, an unknown host): start as before.
      if (h.ok && h.value?.over) {
        limitAsk = h.value;
        return;
      }
    }
    limitAsk = null;
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
        model: runsClaude && chosenModel ? chosenModel : runsCodex && codexModel ? codexModel : null,
        effort: runsClaude && chosenEffort ? chosenEffort : runsCodex && codexEffort ? codexEffort : null,
        profile: runsClaude && chosenProfile.trim() ? chosenProfile.trim() : null,
        agent: chosenKind === 'work' && chosenAgent !== 'claude' ? chosenAgent : null,
        // Step 4.4: asked and confirmed here, so a hub does not refuse it.
        ...(limitConfirmed ? { over_limit_ok: true } : {}),
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
          error = r.error;
        }
      }
      return;
    }
    remember(submittedHost, submittedWorktreeId);
    answerHostProposal(submittedHost);
    if (fleetOptions.length > 0) rememberFleetFor(owner, repo, PRIMARY);
    finished(r.value, submittedHost);
  }

  // The picker's ⌘↵ (project picker spec v2): once this host's worktree
  // list has settled, look ONCE and, if nothing needs a person, submit with
  // what the dialog remembered. The first look is final: a blocked hub, a
  // new worktree without a name, or a scan that failed (`error` /
  // `unlistable` — its empty list would otherwise silently create a
  // generated worktree) leaves the dialog open as if the person had not
  // pressed ⌘↵, and a later reconnect or edit never fires a submit.
  // A ticket start has its own path (`submitTicket`, with a preview the
  // person confirms); the picker never passes a ticket.
  let autostarted = false;
  $effect(() => {
    if (!autostart || autostarted || busy) return;
    if (hostWorktrees.status === 'loading') return;
    autostarted = true;
    if (ticket) return;
    if (hostWorktrees.status !== 'ready') return;
    if (newSessionBlocked || (inNewMode && !newWorktreeName.trim())) return;
    void submit();
  });

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

  // The form rule (`submitKey`, forms/form_frame.ts): ⌘↵ / Ctrl+Enter in
  // any field creates, and so does a bare Enter in a text input — this form
  // is built for Enter-to-create — but not on a checkbox, a radio or a
  // button, and Enter in a textarea is a new line (review w12-2). Escape is
  // handled by <Modal>.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const createHint = submitHint(isMac);
  const rerollKey = isMac ? '⌘R' : 'Ctrl+R';
  let dialogEl: HTMLDivElement | undefined = $state();
  function onKeydown(e: KeyboardEvent) {
    // A nested sheet (Resume, which renders inside this div) answers its own
    // keys: its Enter or ⌘↵ must never also start a new session behind it
    // (review w12-1).
    if (e.key !== 'Enter' || e.defaultPrevented) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest?.('dialog') !== (dialogEl?.closest('dialog') ?? null)) return;
    if (!submitKey(e, 1, isMac)) return;
    e.preventDefault();
    void submit();
  }
  /** Writing help's "Draft agent briefs" (G4.6), off by default. */
  const draftsBriefs = $derived(settingBool($fleetSettings, SETTING_KEYS.workDraftBriefs));
</script>

<Modal label="New session" onclose={onCancel} width="520px">
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog" bind:this={dialogEl} onkeydown={onKeydown}>
  <h3>New session — {owner}/{repo}</h3>
    <ProposedBy
      {proposal}
      field="project"
      testid="new-session-proposed"
      onchange={() => {
        onCancel();
        openNewSessionPicker(initialHost, ticket);
      }}
    />

  {#if fleetOptions.length > 0 && !ticket && !asBackground}
    <div class="fleet-pick" role="radiogroup" aria-label="Fleet" data-testid="new-session-fleet" use:radiogroupKeys>
      <span class="field-label">Fleet</span>
      {#each [{ id: PRIMARY, label: $fleets.find((f) => f.primary)?.label ?? 'This window', color: $fleets.find((f) => f.primary)?.color ?? 'var(--accent)' }, ...fleetOptions.map((c) => ({ id: c.fleet.id, label: c.fleet.label, color: c.fleet.color }))] as f (f.id)}
        <button
          type="button"
          role="radio"
          aria-checked={chosenFleet === f.id}
          class="fleet-chip"
          class:on={chosenFleet === f.id}
          style="--fleet: {f.color}"
          data-testid="new-session-fleet-{f.id}"
          onclick={() => pickFleet(f.id)}>{f.label}</button
        >
      {/each}
    </div>
  {/if}
  {#if runsAs && !ticket && !asBackground}
    <p class="runs-as" data-testid="new-session-profile">Runs as {runsAs}</p>
  {/if}

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
        title="Roll a new name ({rerollKey})"
        aria-label="Roll a new name"
      ><Icon name="dice" size={14} /></button>
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
            {#if ticket && chosenKind === 'work'}
              or
              <button
                type="button"
                class="linkish"
                data-testid="start-another"
                disabled={busy || cannotSubmit !== null}
                onclick={startAnother}>Start another</button
              >
            {/if}
          </span>
        {:else if pastWork && resumeShown && resumeShown.value !== START_FRESH}
          <span class="dup" data-testid="work-past">
            — has previous work:
            <button type="button" class="linkish" data-testid="resume-proposed" onclick={() => openResume(true)}
              >Resume {resumeShownName}</button
            >
          </span>
        {:else if pastWork}
          <span class="dup" data-testid="work-past">
            — has previous work ({pastWorkSummary(pastWork, now * 1000)}).
            <button type="button" class="linkish" data-testid="resume-past" onclick={() => openResume(false)}
              >Resume</button
            >
          </span>
        {:else}
          <span class="muted">— sessions on this branch group under it (sidebar: by work)</span>
        {/if}
      </p>
      {#if pastWork && !duplicateOf && resumeShown}
        {#if resumeShown.value === START_FRESH}
          <ProposedBy
            proposal={{ ...resumeShown, reason: 'start fresh' }}
            field="resume"
            floor={RESUME_OR_NEW_FLOOR}
            changeLabel="Resume instead"
            onchange={() => openResume(false)}
            testid="resume-proposed-by"
          />
        {:else}
          <ProposedBy
            proposal={resumeShown}
            field="resume"
            floor={RESUME_OR_NEW_FLOOR}
            changeLabel="Start fresh instead"
            onchange={startFreshInstead}
            testid="resume-proposed-by"
          />
        {/if}
      {/if}
    {/if}
    {#if ticket}
      <div class="ticket-box" data-testid="ticket-box">
        <p class="work-note">
          <span class="k">ticket</span> <code>{ticket.key}</code>
          {ticket.title}{#if ticket.status_name}<span class="muted"> — {ticket.status_name}</span>{/if}
        </p>
        {#if chosenKind === 'work' && liveConflict && !duplicateOf}
          <p class="work-note" data-testid="ticket-live">
            <span class="dup">{liveConflict.message}</span>
            {#if liveConflictRow}
              <button type="button" class="linkish" data-testid="ticket-live-open" onclick={openLiveConflict}>Open it</button>
              or
            {/if}
            <button
              type="button"
              class="linkish"
              data-testid="start-another"
              disabled={busy || cannotSubmit !== null}
              onclick={startAnother}>Start another</button
            >
          </p>
        {/if}
        {#if chosenKind === 'work' && crossOrg}
          <label class="brief-toggle" data-testid="ticket-cross-org">
            <input type="checkbox" data-testid="ticket-cross-org-on" bind:checked={forceCrossOrg} />
            Start across organisations
          </label>
          <p class="muted small" data-testid="ticket-cross-org-why">{crossOrg}</p>
        {/if}
        {#if chosenKind === 'work'}
          <label class="brief-toggle">
            <input type="checkbox" data-testid="ticket-brief-on" bind:checked={briefOn} />
            Brief Claude with the ticket
          </label>
          {#if briefOn}
            {#if draftMeta || drafting}
              <DraftField
                label="Brief for Claude"
                bind:value={briefDraft}
                model={draftMeta?.model}
                host={draftMeta?.host_alias}
                from={draftMeta ? draftSource(draftMeta) : null}
                busy={drafting}
                rows={8}
                onregenerate={() => void draftTicketBrief(ticket)}
                onclear={clearDraft}
                testid="ticket-brief-draft"
              />
            {:else}
              <textarea
                class="brief"
                aria-label="Brief for Claude"
                data-testid="ticket-brief"
                rows="6"
                value={briefText}
                oninput={(e) => onBriefInput((e.target as HTMLTextAreaElement).value)}
              ></textarea>
              {#if draftsBriefs}<button
                type="button"
                class="btn btn--quiet draft-ask"
                data-testid="ticket-brief-draft-ask"
                disabled={startBlocked != null || !chosenHost}
                title="Write the brief from the ticket and earlier sessions on it, with a model call on {chosenHost || 'the host'}"
                onclick={() => void draftTicketBrief(ticket)}>Draft with Claude</button
              >{/if}
            {/if}
            {#if draftError}
              <p class="draft-error small" role="alert" data-testid="ticket-brief-draft-error">{draftError}</p>
            {/if}
            <p class="muted small">
              Delivered with the first prompt (never typed into the pane), with the ticket's
              latest comments and attachment links read from its tracker. The description is
              the ticket author's text and stays fenced as untrusted.
            </p>
          {/if}
          {#if siblings.length > 0}
            <fieldset class="also-in" data-testid="ticket-also-in">
              <legend>Also start in <span class="muted small">(one session each, same branch)</span></legend>
              {#each siblings as c (c.id)}
                <label class:ai-pre={c.id === proposedSibling && alsoIn.includes(c.id) && !alsoTouched}>
                  <input
                    type="checkbox"
                    data-testid="ticket-also-in-{c.id}"
                    checked={alsoIn.includes(c.id)}
                    onchange={(e) => toggleAlso(c.id, (e.target as HTMLInputElement).checked)}
                  />
                  {c.label}
                </label>
                {#if c.id === proposedSibling && alsoIn.includes(c.id)}
                  <ProposedBy
                    proposal={siblingAsk}
                    field="sibling"
                    changeLabel="Untick"
                    testid="ticket-also-in-proposed"
                    onchange={() => toggleAlso(c.id, false)}
                  />
                {/if}
              {/each}
            </fieldset>
          {/if}
        {/if}
      </div>
    {/if}
    {#if resumeOpen && plannedKey}
      <ResumeDialog
        workKey={plannedKey}
        linkId={resumeLink?.link ?? null}
        sessionId={resumeLink?.session ?? null}
        onclose={() => (resumeOpen = false)}
        onresumed={onResumed}
      />
    {/if}

      <!-- A group, not a labelable control: it is named by aria-labelledby. -->
      <span class="field-label" id="kind-picker-label">Agent</span>
      <div class="kind-row" id="kind-picker" role="group" aria-labelledby="kind-picker-label" data-testid="agent-picker">
        <button
          class="kind-pick"
          class:active={runsClaude}
          aria-pressed={runsClaude}
          data-testid="kind-work"
          onclick={() => onPickKind('work')}
        >
          Claude Code
        </button>
        <button
          class="kind-pick"
          class:active={chosenKind === 'shell'}
          aria-pressed={chosenKind === 'shell'}
          data-testid="kind-shell"
          onclick={() => onPickKind('shell')}
        >
          Shell
        </button>
        {#each agentOptions.filter((c) => c.agent !== 'claude') as c (c.agent)}
          {@const on = chosenKind === 'work' && chosenAgent === c.agent}
          <button
            class="kind-pick"
            class:active={on}
            aria-pressed={on}
            data-testid={`agent-${c.agent}`}
            disabled={!c.enabled}
            title={c.reason ?? undefined}
            onclick={() => onPickAgent(c.agent)}
          >
            {c.label}{#if c.tag}{" "}<span class="soon">{c.tag}</span>{/if}
          </button>
        {/each}
      </div>

    {#if runsCodex}
      <div class="launch-row">
        <div class="launch-field">
          <label for="launch-model">Model</label>
          <select id="launch-model" data-testid="launch-model" bind:value={codexModel}>
            <option value="">Codex default</option>
            {#each CODEX_MODEL_OPTIONS as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </select>
        </div>
        <div class="launch-field">
          <label for="launch-effort">Effort</label>
          <select id="launch-effort" data-testid="launch-effort" bind:value={codexEffort}>
            <option value="">Codex default</option>
            {#each CODEX_EFFORT_OPTIONS as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </select>
        </div>
      </div>
    {:else if runsClaude && !ticket && !asBackground}
      <div class="launch-row">
        <div class="launch-field">
          <label for="launch-model">Model</label>
          <select id="launch-model" data-testid="launch-model" bind:value={chosenModel}>
            <option value="">Host default</option>
            {#each MODEL_OPTIONS.filter((o) => o.value !== 'default') as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </select>
        </div>
        <div class="launch-field">
          <label for="launch-effort">Effort</label>
          <select id="launch-effort" data-testid="launch-effort" bind:value={chosenEffort}>
            <option value="">Host default</option>
            {#each LAUNCH_EFFORT_OPTIONS as o (o.value)}
              <option value={o.value}>{o.label}</option>
            {/each}
          </select>
        </div>
        {#if fleetChoice}
          <!-- This window's logins are its own hub's (review w01-2). -->
          <p class="work-note" data-testid="fleet-login-note">Uses the login {fleetChoice.fleet.label} has on that host.</p>
        {:else if hostLogins && hostLogins.length > 0 && !otherProfile}
          <div class="launch-field">
            <label for="launch-account">Account</label>
            <select
              id="launch-account"
              data-testid="launch-account"
              value={chosenProfile}
              onchange={(e) => onPickLogin(e.currentTarget.value)}
            >
              {#each hostLogins as l (l.profile ?? '')}
                <option value={l.profile ?? ''}>{loginLabel(l, accountName)} · {usedText(l)}</option>
              {/each}
              <option value={OTHER_PROFILE}>Other profile…</option>
            </select>
          </div>
        {:else}
          <div class="launch-field">
            <label for="launch-profile">Login profile</label>
            <input
              id="launch-profile"
              data-testid="launch-profile"
              bind:value={chosenProfile}
              oninput={onTypeProfile}
              placeholder="Host login"
              list="launch-profile-options"
              maxlength="32"
              aria-invalid={profileInvalid}
              title="A name such as work: the session runs under ~/.claude-profiles/<name> on the host, with its own /login. A new profile asks you to log in, in the session."
            />
            <datalist id="launch-profile-options">
              {#each hostProfiles as p (p.name)}
                <option value={p.name}>{p.email ?? (p.account_uuid ? p.name : 'not logged in')}</option>
              {/each}
            </datalist>
          </div>
        {/if}
      </div>
    {/if}

    {#if runsClaude && !ticket}
      <span class="field-label" id="run-picker-label">Run</span>
      <div class="kind-row" id="run-picker" role="group" aria-labelledby="run-picker-label">
        <button
          class="kind-pick"
          class:active={!runBackground}
          aria-pressed={!runBackground}
          data-testid="run-pane"
          onclick={() => (runBackground = false)}
        >
          In a pane
        </button>
        <button
          class="kind-pick"
          class:active={runBackground}
          aria-pressed={runBackground}
          data-testid="run-background"
          onclick={() => (runBackground = true)}
          title="A supervised background session on the host, in this project's checkout, with no pane"
        >
          In background
        </button>
      </div>
      {#if runBackground}
        <label for="bg-prompt">First prompt</label>
        <textarea id="bg-prompt" data-testid="bg-prompt" rows="3" bind:value={bgPrompt} placeholder="What should Claude work on?"></textarea>
        <p class="work-note" data-testid="bg-note">
          Runs supervised on {chosenHost} in {repo}'s checkout, with no pane; it shows in the list when it starts.
        </p>
      {/if}
    {/if}

    {#if chosenKind === 'shell'}
      <label for="start-command">start command (optional)</label>
      <input
        id="start-command"
        data-testid="start-command"
        bind:value={startCommand}
        placeholder="e.g. pnpm test"
      />
    {/if}

    {#if fleetChoice}
      <label for="fleet-host">Host on {fleetChoice.fleet.label}</label>
      <select id="fleet-host" data-testid="new-session-fleet-host" bind:value={fleetHost}>
        {#each fleetHosts as h (h.alias)}
          <option value={h.alias}>{h.alias}{h.reachable ? '' : ' (unreachable)'}</option>
        {/each}
      </select>
      {#if fleetHosts.length === 0}
        <p class="wt-status">{fleetChoice.fleet.label} lists no hosts yet.</p>
      {:else if fleetChoice.projectId === null}
        <p class="wt-status" data-testid="new-session-fleet-clone">
          {fleetChoice.fleet.label} doesn't have {owner}/{repo} yet. Create clones it onto {fleetHost || 'the host'} first.
        </p>
      {/if}
      <label for="fleet-branch">new branch (optional)</label>
      <input
        id="fleet-branch"
        data-testid="new-session-fleet-branch"
        bind:value={fleetBranch}
        placeholder="empty = the project's checkout"
      />
    {:else}
    <HostChips
      active={chosenHost}
      labelId="new-session-host-label"
      showUsage
      {now}
      {locale}
      {timeZone}
      selectedAccount={chosenLoginAccount}
      proposedHost={hostProposed?.value ?? null}
      onpick={(alias) => {
        pickHost(alias);
        nameOverride = null;
      }}
    />
    {#if hostProposed && hostProposed.value === chosenHost}
      <ProposedBy
        proposal={hostProposed}
        field="host"
        floor={HOST_PLACEMENT_FLOOR}
        testid="new-session-host-proposed"
        onchange={changeProposedHost}
      />
    {:else if hostCorrection}
      <p class="correction" data-testid="new-session-host-corrected">
        {correctionLine(hostCorrection.from, hostCorrection.to, 'when you start')}
      </p>
    {/if}

    {#if !asBackground}
    <!-- Not a <label>: the picker is a listbox, which a label cannot name
         (it names itself with ariaLabel). A background run takes none of
         the worktree rows: it starts in the project's checkout (w12-9). -->
    <span class="field-label" aria-hidden="true">Worktree</span>
    {#if hostWorktrees.status === 'error' && hostWorktrees.error}
      <div class="wt-status err" role="alert" data-testid="wt-status">
        <span data-testid="wt-status-text"
          >Couldn't list the worktrees on {chosenHost}. {errorSentence(hostWorktrees.error)}</span
        >
        <button type="button" class="wt-rescan" data-testid="wt-scan-again" onclick={() => rescans++}>Scan again</button>
        <details class="wt-details">
          <summary>Details</summary>
          <code data-testid="wt-status-details">{errorDetail(hostWorktrees.error)}</code>
        </details>
      </div>
    {:else if worktreeStatus}
      <p class="wt-status" data-testid="wt-status">{worktreeStatus}</p>
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
    {/if}
    {/if}

    {#if !asBackground}
    <label for="session-name">tmux name</label>
    <input
      id="session-name"
      data-testid="new-session-name"
      value={name}
      oninput={(e) => onNameInput((e.target as HTMLInputElement).value)}
      placeholder="empty = let fleet pick one"
    />
    {#if !fleetChoice}
    <p class="preview" data-testid="path-preview" title={pathPreview}>
      <span class="k">cwd</span> <code>{pathPreview}</code>
    </p>
    {/if}
    {/if}
  </div>

  <!-- Outside the scrolling fields, beside the buttons: a failure or the
       start's progress is never below the fold (review w12-6). -->
  {#if error}
    <div class="err" role="alert" data-testid="new-session-error" bind:this={errorEl}>
      {#if typeof error === 'string'}
        {error}
      {:else}
        <span data-testid="new-session-error-text">{errorSentence(error)}</span>
        <details class="wt-details">
          <summary>Details</summary>
          <code data-testid="new-session-error-details">{errorDetail(error)}</code>
        </details>
      {/if}
    </div>
  {/if}
  {#if busy && $creatingStart}
    <!-- Redesign step 5.13: the backend reports the create's worktree,
         tmux and agent steps (`start:progress`) while it is in flight; the
         dialog then closes into the same Pulse in the new session's
         conversation, which waits on the agent's first status. -->
    {@const c = $creatingStart}
    <div class="pulse-row"><StartPulse start={c} title="Starting {c.name || 'a session'} on {c.host_alias}" /></div>
  {/if}

  <div class="actions">
    <span class="hint">↵ or {createHint.label} create · {rerollKey} re-roll</span>
    {#if limitAsk && limitAsk.chosen}
      <div class="limit-ask" role="alert" data-testid="limit-ask">
        <span
          >{accountName(limitAsk.chosen.account_uuid)} is at {Math.round(limitAsk.chosen.used_pct ?? 0)}% of its
          limit (this asks from {limitAsk.pause_at_pct}%).</span
        >
        {#if limitAsk.suggestion}
          {@const s = limitAsk.suggestion}
          <button class="primary" data-testid="limit-use-suggestion" onclick={() => useLogin(s)}
            >Use {loginLabel(s, accountName)} · {usedText(s)}</button
          >
        {/if}
        <button data-testid="limit-start-anyway" onclick={startAnyway}>Start anyway</button>
      </div>
    {/if}
    <button onclick={onCancel} disabled={busy}>Cancel</button>
    {#if hubCreateNote && !fleetChoice}
      <span class="hub-create-note" data-testid="hub-create-note" title={hubCreateNote}>{hubCreateNote}</span>
    {:else if busy && createController}
      <button type="button" data-testid="cancel-create" onclick={cancelCreate}>Cancel creation</button>
    {:else if busy}
      <!-- Nothing to cancel: a ticket, multi-repo, background or fleet start
           and the limit check run to the end (review w12-3). -->
      <span class="hub-create-note" data-testid="create-busy" role="status">Starting…</span>
    {:else}
      <button
        class="primary"
        onclick={() => void submit()}
        data-testid="create-btn"
        disabled={cannotSubmit !== null}
        title={cannotSubmit ?? ''}
        aria-keyshortcuts={createHint.aria}
      >{fleetChoice ? `Create on ${fleetChoice.fleet.label}` : asBackground ? 'Start in background' : ticket && chosenKind === 'work' ? 'Start work' : 'Create'}</button>
    {/if}
  </div>
</div>
</Modal>

<style>
  .soon {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .limit-ask {
    flex: 1 1 100%;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.5rem;
    border: 1px solid color-mix(in srgb, var(--usage-warn) 45%, transparent);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-size: var(--text-2xs);
  }
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
  .dialog h3 { margin: 0 0 0.3rem 0; font-size: var(--text-sm); flex: 0 0 auto; }
  .runs-as {
    margin: 0 0 var(--space-2);
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .fleet-pick {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-wrap: wrap;
    margin-bottom: var(--space-2);
  }
  .fleet-chip {
    border: 1px solid var(--border);
    border-left: 3px solid var(--fleet);
    border-radius: var(--radius-sm);
    background: none;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    padding: 2px var(--space-2);
    cursor: pointer;
  }
  .fleet-chip.on {
    background: color-mix(in srgb, var(--fleet) 16%, transparent);
    border-color: var(--fleet);
  }
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
  label, .field-label { font-size: var(--text-2xs); color: var(--fg-muted); text-transform: uppercase; }
  input {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: var(--radius-sm);
    min-width: 0;
  }
  .name-row { display: flex; gap: 0.3rem; }
  .work-note {
    margin: 0;
    font-size: var(--text-2xs);
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
    font-size: var(--text-md);
    line-height: 1;
    padding: 0.2rem 0.45rem;
    border: 1px solid var(--border);
    background: transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .dice:hover { border-color: var(--accent); }
  .kind-row { display: flex; gap: 0.3rem; }
  .launch-row { display: flex; gap: 0.6rem; }
  .launch-field { flex: 1 1 0; min-width: 0; display: flex; flex-direction: column; gap: 0.2rem; }
  .launch-field select {
    font: inherit;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    color: var(--fg);
    min-width: 0;
  }
  .kind-pick {
    font-size: var(--text-2xs);
    padding: 0.2rem 0.7rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  .kind-pick.active { color: var(--fg); border-color: var(--accent); }
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
  /* Pinned with the buttons, outside the scrolling fields (review w12-6). */
  .dialog > .err,
  .pulse-row { flex: 0 0 auto; }
  .wt-status { font-size: var(--text-2xs); color: var(--fg-muted); margin: 0 0 0.2rem; }
  .wt-status.err { color: var(--danger); }
  .wt-rescan { font: inherit; color: var(--fg); background: none; border: 0; padding: 0; margin-left: 0.4rem; text-decoration: underline; cursor: pointer; }
  .wt-details { color: var(--fg-muted); }
  .wt-details summary { cursor: pointer; }
  .wt-details code { display: block; overflow-wrap: anywhere; }
  .actions {
    display: flex;
    gap: 0.4rem;
    justify-content: flex-end;
    align-items: center;
    flex: 0 0 auto;
    padding-top: 0.2rem;
    border-top: 1px solid var(--border);
  }
  .actions .hint { margin-right: auto; font-size: var(--text-2xs); color: var(--fg-muted); }
  .hub-create-note { font-size: var(--text-2xs); color: var(--fg-muted); text-align: right; }
  .actions button {
    font-size: var(--text-xs);
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: var(--radius-sm);
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
    font-size: var(--text-2xs);
  }
  .brief {
    font: inherit;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--text-2xs);
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
  }
  .small {
    font-size: var(--text-2xs);
  }
  .draft-ask {
    align-self: flex-start;
  }
  .draft-error {
    margin: 0;
    color: var(--danger);
  }
  .correction {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
</style>
