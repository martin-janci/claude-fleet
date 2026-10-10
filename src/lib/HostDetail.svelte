<script lang="ts">
  // The Hosts view's detail pane (spec: "The Hosts view" → Detail sections).
  // In order: header, usage, sessions, integration, danger. ("Today" is left
  // out: the frontend has per-session lifetime totals only, no per-day figure,
  // and this view adds no new data path.) Action safety per the spec:
  // reversible actions are plain buttons (hide shows an Undo toast);
  // `Rotate token…` and `Remove host…` sit at the bottom, have no keyboard
  // shortcut, and confirm with Cancel focused, stating the consequence.
  import HostOffline from './states/HostOffline.svelte';
  import Icon from './kit/Icon.svelte';
  import KeyValue from './kit/KeyValue.svelte';
  import Meter from './kit/Meter.svelte';
  import type { HostRow } from './hosts';
  import { deleteHost, setHostHarnesses, codexModeOf, harnessesFor, type HarnessMode } from './hosts';
  import type { AccountRow } from './accounts';
  import type { AccountUsageSnapshot } from './account_usage_store';
  import type { HostTokenInfo, TokenMode } from './mcp';
  import {
    adoptSession,
    restoreHostSessions,
    discoverLostSessions,
    newSessionAbortable,
    type RestorePlanEntry,
    type LostCandidate,
    type SessionRow,
  } from './sessions';
  import { selectSessionExplicitly } from './selection';
  import { claudeStatusLabel, stuckStatus } from './attention';
  import { formatAge, hookHealthLabel, type HookHealth } from './hook_health';
  import { shortAge } from './session_status';
  import { hideHostWithUndo, rotateToken, setTokenMode, showHost, viewHostSessions } from './host_actions';
  import { pushError, push } from './toasts';
  import {
    diskMeter,
    healthLine,
    removeHostMessage,
    rotateTokenMessage,
    versionAge,
    type HostAttention,
  } from './hosts_view';
  import { hubStatus, hubBlock, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { bulkTargets, sessionBlocked, sessionIdBlocked } from './share';
  import AccountNickname from './AccountNickname.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { isRestorable } from './lost_fold';
  import type { HostTidyHint } from './hosts_table';
  import EmbedSlot from './pages/EmbedSlot.svelte';
  import { inventory } from './assets';
  import { provisionHost } from './mcp';
  import { CHECK_GLYPH, checklistLoaderText, checklistRows, hostChecks, needsReprovision, runHostCheck } from './host_check';
  import Loader from './Loader.svelte';
  import AgentInstallAction from './AgentInstallAction.svelte';
  import LostTargetForm from './LostTargetForm.svelte';
  import {
    ignoredConversations,
    ignoredPanes,
    isOutsideFleet,
    needsRestoreInto,
    otherConversations,
    placeTranscript,
    setConversationIgnored,
    setPaneIgnored,
  } from './lost_found';
  import AddAccountDialog from './AddAccountDialog.svelte';
  import { openNewSessionPicker } from './switcher_request';
  import { requestAssetsView } from './app_views';
  import { savedWithUndo } from './forms/form_frame';
  import { linkSessionWork } from './work';

  let {
    host,
    account,
    snapshot,
    sharedWith,
    hostSessions,
    token,
    tokensLoaded,
    hook,
    attention,
    now,
    locale,
    timeZone,
    suppressUnavailable = false,
    editingNickname,
    probing = false,
    detailEl = $bindable(),
    oneditstart,
    oneditdone,
    onreprobe,
    onrefreshusage,
    onnewsession,
    tidyHint = null,
    onreviewtidy,
    hubVersion = null,
  }: {
    host: HostRow;
    account: AccountRow | null;
    snapshot: AccountUsageSnapshot | null;
    sharedWith: string[];
    hostSessions: SessionRow[];
    token: HostTokenInfo | null;
    tokensLoaded: boolean;
    hook: HookHealth;
    attention: HostAttention | null;
    now: number;
    locale?: string;
    timeZone?: string;
    suppressUnavailable?: boolean;
    editingNickname: boolean;
    probing?: boolean;
    detailEl?: HTMLElement;
    oneditstart: () => void;
    oneditdone: () => void;
    onreprobe: () => void;
    onrefreshusage: () => void;
    /** Orbit Fleet 4.7: "New session here". Absent: the button is not shown. */
    onnewsession?: () => void;
    /** G4.5: what a Tidy clean up would free on this host. */
    tidyHint?: HostTidyHint | null;
    /** "Review in Tidy": opens the Tidy-up sheet on these sessions. */
    onreviewtidy?: (sessionIds: number[]) => void;
    /** The version a fleet-agent should match (the hub's when paired). */
    hubVersion?: string | null;
  } = $props();

  // Orbit Fleet 4.7: the health checklist. Run on demand (an SSH round trip
  // per host; never on every selection move), remembered per host.
  let checking = $state(false);
  // Show sessions (step 3.14's offline host): to this pane's own list.
  let sessionsBlock = $state<HTMLElement | null>(null);
  // M15 G7.12: the detail's tabs (HostDetail board). Every panel stays
  // mounted and hides, so a draft or a running find survives a look at
  // another tab.
  type DetailTab = 'overview' | 'sessions' | 'lost' | 'provisioning';
  let tab = $state<DetailTab>('overview');
  function showSessions() {
    tab = 'sessions';
    sessionsBlock?.scrollIntoView?.({ block: 'start' });
    sessionsBlock?.querySelector<HTMLElement>('[data-testid="detail-session"]')?.focus();
  }
  let provisioning = $state(false);
  const lastCheck = $derived($hostChecks.get(host.alias) ?? null);
  // Orbit Fleet 4.9: a paired desktop's hub accepts agents, so an SSH host's
  // fleet-agent row offers the install job.
  const checklist = $derived(
    checklistRows({ host, check: lastCheck, inventory: $inventory, hubVersion, agentsAccepted: $hubStatus.remote }),
  );
  const checkBlocked = $derived(hubBlock('check_host', $hubStatus));
  const provisionBlocked = $derived(hubBlock('provision_hosts', $hubStatus));
  const reprovisionAdvised = $derived(needsReprovision(checklist, host));
  // Orbit Fleet 4.13: the Hex field beside what is running, Layout: New only.
  const liveText = $derived(checklistLoaderText(host.alias, checking, provisioning));

  async function runChecks() {
    if (checkBlocked !== null || checking) return;
    checking = true;
    const r = await runHostCheck(host.alias);
    checking = false;
    if (!r.ok) pushError(r.error, `Checking ${host.alias} failed`);
  }

  async function reprovision() {
    if (provisionBlocked !== null || provisioning) return;
    provisioning = true;
    const r = await provisionHost(host.alias);
    provisioning = false;
    if (!r.ok) {
      pushError(r.error, `Re-provisioning ${host.alias} failed`);
      return;
    }
    const mine = r.value.find((x) => x.host === host.alias);
    if (mine && mine.status === 'failed') {
      push({ kind: 'error', message: `Re-provisioning ${host.alias} failed: ${mine.detail ?? 'no detail'}` });
      return;
    }
    push({ kind: 'success', message: `${host.alias} re-provisioned` });
    if (checkBlocked === null) void runChecks();
  }

  let confirm = $state<'rotate' | 'remove' | 'restore' | null>(null);
  let busy = $state(false);

  const isLocal = $derived(host.alias === 'local');
  /** The $HOME disk meter, null until the host was sampled. */
  const disk = $derived(diskMeter(host));
  // The header's facts, in the kit's KeyValue (manual: KeyValue).
  const facts = $derived([
    ...(host.ssh_alias ? [{ label: 'ssh', value: host.ssh_alias, mono: true, testid: 'detail-ssh' }] : []),
    ...(host.transport === 'agent' ? [{ label: 'transport', value: 'agent', testid: 'detail-transport' }] : []),
    {
      label: 'last ping',
      value: host.last_pinged_at ? `${formatAge(now - host.last_pinged_at)} ago` : 'never',
      tnum: true,
      testid: 'detail-ping',
    },
    { label: 'claude', content: claudeFact, tnum: true },
    { label: 'tmux', value: host.tmux_version ?? '—', tnum: true },
  ]);

  // Sessions the backend marked lost (host reboot / tmux server restart) that
  // still carry a Claude conversation to resume. `bg`/`external` rows have no
  // fleet-managed tmux pane to restore into.
  const restorable = $derived(hostSessions.filter(isRestorable));
  /**
   * Multi-user M1 (F2b): "Restore n lost sessions…" was gated on NOTHING —
   * neither the hub's half nor the access half — and `restore_host_sessions`
   * is spec §4.3's `own` tier, the batch form of `recreate_session`. A host's
   * lost rows are not all one person's, so this narrows PER TARGET rather than
   * answering once for the button: `bulkTargets` is the same shape Sidebar's
   * select mode and TidyReview use, and the dialog below says how many rows
   * were left out so the count on the button is never a silent truncation.
   */
  const restorableMine = $derived(bulkTargets(restorable, 'restore_host_sessions', $sessionBlocked));
  const restoreNotMine = $derived(restorable.length - restorableMine.length);
  /** Why the button is dead: the hub's own half first (it ROUTES, so a down
   *  link blocks it), then the access half — and, when every lost row on the
   *  host belongs to someone else, that sentence, since the hub half would
   *  answer `null` and leave the button live over nothing this client owns. */
  const restoreBlocked = $derived(
    hubActionBlocked('restore_host_sessions', $hubStatus, $hubConnection) ??
      (restorableMine.length === 0
        ? ($sessionBlocked(restorable[0], 'restore_host_sessions') ??
          'None of the lost sessions on this host are yours to restore.')
        : null),
  );
  /** The ids this client may actually restore, as a set, so the plan the
   *  backend answers with can be filtered by it without a second narrowing. */
  const restorableMineIds = $derived(new Set(restorableMine.map((s) => s.id)));

  let restorePlan = $state<RestorePlanEntry[] | null>(null);
  let restoreError = $state<string | null>(null);
  let restoreSummary = $state<{ ok: number; total: number; failures: { name: string; error: string }[] } | null>(
    null,
  );

  // Find lost conversations: transcripts the host has that fleet has no row
  // for (discover_lost_sessions), each optionally resumable into a new
  // fleet-managed session (new_session with resume_claude_session_id).
  let discoverBusy = $state(false);
  let discoverError = $state<string | null>(null);
  let discoverList = $state<LostCandidate[] | null>(null);
  let resumingId = $state<string | null>(null);
  let resumedIds = $state<Set<string>>(new Set());
  let resumeErrors = $state<Record<string, string>>({});

  // Ignore (gap plan G2.7): a found conversation a person does not want
  // back is left out of later searches on this device (`lost_found.ts`).
  let ignored = $state<ReadonlySet<string>>(new Set());
  let showIgnored = $state(false);
  $effect(() => {
    ignored = ignoredConversations(host.alias);
  });
  // "Find another…" (G4.5) on a lost row: the search, without that row's own
  // conversation and with the ones from its folder first.
  let findingFor = $state<SessionRow | null>(null);
  const candidateList = $derived(
    discoverList && findingFor
      ? otherConversations(discoverList, {
          claude_session_id: findingFor.claude_session_id,
          project_id: findingFor.project_id,
        })
      : discoverList,
  );
  const shownCandidates = $derived(
    candidateList ? (showIgnored ? candidateList : candidateList.filter((c) => !ignored.has(c.claude_session_id))) : null,
  );
  const ignoredCount = $derived(discoverList ? discoverList.filter((c) => ignored.has(c.claude_session_id)).length : 0);
  function setIgnored(c: LostCandidate, on: boolean) {
    setConversationIgnored(host.alias, c.claude_session_id, on);
    ignored = ignoredConversations(host.alias);
  }
  function ignoreCandidate(c: LostCandidate) {
    restoringId = null;
    setIgnored(c, true);
    savedWithUndo(`Ignored ${c.git_branch ?? c.cwd} on this device`, () => setIgnored(c, false));
  }

  function rankLabel(hint: LostCandidate['rank_hint']): string | null {
    switch (hint) {
      case 'before_boot':
        return 'before reboot';
      case 'after_boot':
        return 'since boot';
      case 'stale':
        return 'older';
      default:
        return null;
    }
  }

  async function findAnother(s: SessionRow) {
    findingFor = s;
    tab = 'lost';
    await runDiscover();
  }

  async function onDiscoverClick() {
    findingFor = null;
    tab = 'lost';
    await runDiscover();
  }

  async function runDiscover() {
    discoverError = null;
    discoverList = null;
    resumedIds = new Set();
    resumeErrors = {};
    discoverBusy = true;
    const r = await discoverLostSessions(host.alias);
    discoverBusy = false;
    if (!r.ok) {
      discoverError = r.error.message;
      return;
    }
    discoverList = r.value;
  }

  async function onResumeCandidate(c: LostCandidate) {
    if (!c.resumable || c.project_id === null || c.derived_tmux_name === null) return;
    // Re-asked at the write: the discovered list stays on screen, and nothing
    // in it moves when a grant is narrowed.
    const refused = candidateBlocked(c);
    if (refused !== null) {
      resumeErrors = { ...resumeErrors, [c.claude_session_id]: refused };
      return;
    }
    const { [c.claude_session_id]: _dropped, ...rest } = resumeErrors;
    resumeErrors = rest;
    resumingId = c.claude_session_id;
    const r = await newSessionAbortable({
      host_alias: host.alias,
      project_id: c.project_id,
      worktree_id: c.worktree_id,
      name: c.derived_tmux_name,
      resume_claude_session_id: c.claude_session_id,
    });
    resumingId = null;
    if (r.ok) {
      resumedIds = new Set(resumedIds).add(c.claude_session_id);
    } else {
      resumeErrors = { ...resumeErrors, [c.claude_session_id]: r.error.message };
    }
  }

  // Hiding, removing and re-tokening a host are fleet administration, which
  // the hub refuses to a paired client (`enforce_admin`) and which this app
  // guards with `E_LOCAL_ONLY` before it even asks. Disabled with the reason
  // in the tooltip rather than left to fail at the click: the button would
  // otherwise look like a button that works.
  const adminBlocked = $derived(hubBlock('remove_host', $hubStatus));
  // Neither has a hub tool either: the nickname lives in the hub's own
  // database, and a usage refresh SSHes to the host from here.
  const nicknameBlocked = $derived(hubBlock('set_account_nickname', $hubStatus));
  const refreshUsageBlocked = $derived(hubBlock('refresh_account_usage', $hubStatus));
  // probe_host routes, so it only needs the live connection to be up.
  const reprobeBlocked = $derived(hubActionBlocked('probe_host', $hubStatus, $hubConnection));
  // `HostsView` never fetches `list_host_tokens` on a hub client (it is
  // local-only), so `token` is always null and `tokensLoaded` never turns
  // true there — without this, the empty-token line below would show "…"
  // forever instead of a real answer.
  const hostTokensBlocked = $derived(hubBlock('host_tokens', $hubStatus));

  // Which harnesses the asset catalog syncs here (F3a) routes to the hub's
  // catalog_admin, so a paired desktop only needs the live link.
  const harnessBlocked = $derived(hubActionBlocked('catalog_set_host_harnesses', $hubStatus, $hubConnection));

  // M15 G7.12: the integrations are one form (Accounts forms board, Host
  // integrations): a pick is a draft until Save, and Discard drops it. A
  // draft belongs to its host; another host's detail starts clean.
  let codexDraft = $state<{ alias: string; mode: HarnessMode } | null>(null);
  let tokenDraft = $state<{ alias: string; mode: TokenMode } | null>(null);
  const codexValue = $derived(codexDraft?.alias === host.alias ? codexDraft.mode : codexModeOf(host));
  const tokenValue = $derived(tokenDraft?.alias === host.alias ? tokenDraft.mode : token?.mode);
  const integrationDirty = $derived(
    (codexDraft?.alias === host.alias && codexDraft.mode !== codexModeOf(host)) ||
      (tokenDraft?.alias === host.alias && !!token && tokenDraft.mode !== token.mode),
  );

  function discardIntegrations() {
    codexDraft = null;
    tokenDraft = null;
  }

  async function saveIntegrations() {
    busy = true;
    if (codexDraft?.alias === host.alias && codexDraft.mode !== codexModeOf(host)) {
      const r = await setHostHarnesses(host.alias, harnessesFor(codexDraft.mode));
      if (!r.ok) pushError(r.error, 'Codex setting not changed');
      else codexDraft = null;
    }
    if (tokenDraft?.alias === host.alias && token && tokenDraft.mode !== token.mode) {
      const r = await setTokenMode(host.alias, tokenDraft.mode);
      if (!r.ok) pushError(r.error, 'Token mode not changed');
      else tokenDraft = null;
    }
    busy = false;
  }

  // Resume is a `new_session` carrying `resume_claude_session_id`, and
  // `new_session` ROUTES: a paired desktop resumes through the hub like any
  // other routed mutation, so the live link being down blocks it — the same
  // gate `reprobeBlocked` uses.
  const resumeHubBlocked = $derived(hubActionBlocked('new_session', $hubStatus, $hubConnection));

  /**
   * …and the access half, which the hub half cannot answer (multi-user M1,
   * F2c). "Find lost conversations" lists the host's Claude TRANSCRIPTS, not
   * fleet's rows: `discover_lost_sessions` reads `~/.claude/projects` on the
   * machine, which holds every person's conversations who has ever worked
   * there. Resuming one adopts it into a session of this person's — the
   * transcript-acquisition path the milestone's durable conversation-owner
   * record exists to refuse.
   *
   * `new_session` deliberately has NO `SESSION_TIER` row, so the tier table is
   * not the mechanism here: a tier answers "what may I do to an EXISTING
   * session", and `new_session` is how a session comes into being — giving it a
   * row would demand a gate on every creation path in the app, each of which
   * has no session to ask about. What the table does supply is a comparable
   * question for the SOURCE: a candidate that fleet already has a row for
   * (`existing_session_id`) is judged against that row at the `own` tier, with
   * `recreate_session` as the question, because adopting a transcript into a
   * new pane is exactly what `recreate_session` does.
   *
   * A candidate with no row at all resolves to `null`, which
   * `$sessionIdBlocked` fails closed on everywhere but a standalone desktop —
   * the right answer, since an orphaned transcript on a shared host is the
   * case we cannot vouch for at all. A single-user install is unchanged.
   */
  function candidateBlocked(c: LostCandidate): string | null {
    return resumeHubBlocked ?? $sessionIdBlocked(c.existing_session_id, 'recreate_session');
  }

  // Orbit Fleet 4.12: Lost and found with proposals. A pane somebody started
  // by hand is adopted into a project; a found conversation that cannot
  // resume where it ran is restored into one. Both forms are prefilled
  // (LostTargetForm) and both confirm.
  const outsideAll = $derived(hostSessions.filter(isOutsideFleet));
  // Ignore (G4.5): a pane somebody keeps outside fleet on purpose is left out
  // of this list on this device; the pane itself is untouched.
  let paneIgnored = $state<ReadonlySet<string>>(new Set());
  let showIgnoredPanes = $state(false);
  $effect(() => {
    paneIgnored = ignoredPanes(host.alias);
  });
  const outsidePanes = $derived(showIgnoredPanes ? outsideAll : outsideAll.filter((p) => !paneIgnored.has(p.tmux_name)));
  /** The Lost & found tab's count: panes fleet did not start, lost sessions
   *  it can restore, and conversations a find turned up that fleet has not. */
  const lostCount = $derived(
    outsideAll.filter((p) => !paneIgnored.has(p.tmux_name)).length +
      restorable.length +
      (discoverList ?? []).filter((c) => c.existing_session_id === null && !ignored.has(c.claude_session_id)).length,
  );
  const ignoredPaneCount = $derived(outsideAll.filter((p) => paneIgnored.has(p.tmux_name)).length);
  function setPaneIgnoredHere(p: SessionRow, on: boolean) {
    setPaneIgnored(host.alias, p.tmux_name, on);
    paneIgnored = ignoredPanes(host.alias);
  }
  function ignorePane(p: SessionRow) {
    if (adoptingId === p.id) adoptingId = null;
    setPaneIgnoredHere(p, true);
    savedWithUndo(`Ignored ${p.tmux_name} on this device`, () => setPaneIgnoredHere(p, false));
  }

  // + Add account… (G2.9 from the host, G4.5): the wizard with this host picked.
  let addingAccount = $state(false);
  let adoptingId = $state<number | null>(null);
  let restoringId = $state<string | null>(null);
  const adoptHubBlocked = $derived(hubActionBlocked('adopt_session', $hubStatus, $hubConnection));

  function adoptBlocked(s: SessionRow): string | null {
    return adoptHubBlocked ?? $sessionBlocked(s, 'adopt_session');
  }

  async function adoptInto(s: SessionRow, projectId: number | null): Promise<string | null> {
    const refused = adoptBlocked(s);
    if (refused !== null) return refused;
    const r = await adoptSession(s.id, projectId);
    if (!r.ok) return r.error.message;
    adoptingId = null;
    push({ kind: 'success', message: `${s.tmux_name} adopted` });
    return null;
  }

  async function restoreInto(c: LostCandidate, projectId: number | null, ticket: string | null): Promise<string | null> {
    if (projectId === null) return 'Pick a project to restore it into.';
    const refused = candidateBlocked(c);
    if (refused !== null) return refused;
    const placed = await placeTranscript({
      host_alias: host.alias,
      claude_session_id: c.claude_session_id,
      project_id: projectId,
    });
    if (!placed.ok) return placed.error.message;
    const r = await newSessionAbortable({
      host_alias: host.alias,
      project_id: projectId,
      worktree_id: null,
      name: placed.value.tmux_name,
      resume_claude_session_id: c.claude_session_id,
    });
    if (!r.ok) return r.error.message;
    restoringId = null;
    resumedIds = new Set(resumedIds).add(c.claude_session_id);
    // J10: the ticket the branch names, when the person kept it ticked.
    if (ticket !== null) {
      const linked = await linkSessionWork(r.value.id, { key: ticket });
      if (!linked.ok) push({ kind: 'error', message: `Restored, but not linked to ${ticket}: ${linked.error.message}` });
    }
    return null;
  }

  // The restore plan may hold only skips (e.g. the fleet controller, which
  // needs an explicit forced recreate): then there is nothing to confirm.
  const restoreCount = $derived(
    (restorePlan ?? []).filter((e) => e.action === 'restore' && restorableMineIds.has(e.session_id)).length,
  );
  /** Rows the plan would restore that are somebody else's — named in the
   *  dialog rather than silently dropped from the count. */
  const restorePlanNotMine = $derived(
    (restorePlan ?? []).filter((e) => e.action === 'restore' && !restorableMineIds.has(e.session_id)).length,
  );

  function sessionName(s: SessionRow): string {
    return s.friendly_name?.trim() || s.tmux_name;
  }

  function sessionState(s: SessionRow): string {
    if (s.stuck_kind) return stuckStatus(s.stuck_kind);
    if (s.status === 'ghost') return 'ghost';
    return claudeStatusLabel(s.claude_status) || s.status;
  }

  async function onHideToggle() {
    busy = true;
    if (host.hidden) await showHost(host.alias);
    else await hideHostWithUndo(host.alias);
    busy = false;
  }

  async function confirmRotate() {
    const alias = host.alias;
    busy = true;
    const r = await rotateToken(alias);
    busy = false;
    confirm = null;
    if (r.ok) {
      push({ kind: 'success', message: `${alias} has a new control-API token.` });
      if (r.value.warning) push({ kind: 'warning', message: `${alias}: ${r.value.warning}`, sticky: true });
    } else pushError(r.error, `Rotate token for ${alias} failed`);
  }

  async function confirmRemove() {
    const alias = host.alias;
    busy = true;
    const r = await deleteHost(alias);
    busy = false;
    confirm = null;
    if (r.ok) push({ kind: 'info', message: `${alias} removed.` });
    else pushError(r.error, `Remove ${alias} failed`);
  }

  async function onRestoreClick() {
    // Re-asked at the call, not only on the button: a grant can be narrowed
    // between the render that enabled it and the click (and the dry run is
    // itself `restore_host_sessions`).
    if (restoreBlocked !== null) return;
    restoreError = null;
    restoreSummary = null;
    busy = true;
    const r = await restoreHostSessions(host.alias, { dryRun: true });
    busy = false;
    if (!r.ok) {
      restoreError = r.error.message;
      return;
    }
    restorePlan = r.value.plan;
    confirm = 'restore';
  }

  function cancelRestore() {
    confirm = null;
    restorePlan = null;
  }

  async function confirmRestore() {
    // Re-asked here too: the confirm dialog stays open, so a revoke can land
    // between the plan and the click. And the plan comes from the BACKEND,
    // which plans for the host and not for this caller — so the ids are
    // narrowed again against what this client may restore.
    if (restoreBlocked !== null) return;
    const alias = host.alias;
    const ids = (restorePlan ?? [])
      .filter((e) => e.action === 'restore' && restorableMineIds.has(e.session_id))
      .map((e) => e.session_id);
    if (ids.length === 0) {
      confirm = null;
      restorePlan = null;
      return;
    }
    busy = true;
    const r = await restoreHostSessions(alias, { sessionIds: ids });
    busy = false;
    confirm = null;
    restorePlan = null;
    if (r.ok) {
      const results = r.value.results;
      restoreSummary = {
        ok: results.filter((x) => x.ok).length,
        total: results.length,
        failures: results
          .filter((x) => !x.ok)
          .map((x) => ({ name: x.tmux_name, error: x.error ?? 'unknown error' })),
      };
    } else {
      restoreError = r.error.message;
    }
  }
</script>

{#snippet claudeFact()}
  {host.claude_version ?? '—'} <span class="muted" data-testid="detail-claude-age">{versionAge(host, now)}</span>
{/snippet}

<section
  bind:this={detailEl}
  class="host-detail"
  tabindex="-1"
  aria-label="{host.alias} details"
  data-testid="host-detail"
  data-alias={host.alias}
>
  <!-- 1. Header -->
  <header class="head">
    <div class="title-row">
      <h2 data-testid="detail-alias">{host.alias}</h2>
      <span class="status" class:off={!host.reachable} data-testid="detail-status"
        >{host.reachable ? '● online' : '○ offline'}</span
      >
      {#if host.hidden}<span class="muted">hidden</span>{/if}
      <button
        type="button"
        class="small"
        onclick={() => viewHostSessions(host.alias)}
        data-testid="detail-view-sessions"
        ><kbd>s</kbd> View sessions</button
      >
      <button
        type="button"
        class="small"
        onclick={onreprobe}
        disabled={probing || reprobeBlocked !== null}
        title={reprobeBlocked ?? ''}
        data-testid="detail-reprobe"
        >{#if probing}probing…{:else}<kbd>r</kbd> Re-probe{/if}</button
      >
      <button
        type="button"
        class="small"
        title="Sign in to another Claude account on this host"
        data-testid="detail-add-account"
        onclick={() => (addingAccount = true)}>+ Add account…</button
      >
    </div>
    {#if addingAccount}
      <AddAccountDialog host={host.alias} onclose={() => (addingAccount = false)} />
    {/if}
    {#if !host.reachable && !isLocal}
      <!-- The states kit: an offline host is said here, in its own pane.
           "Last answered" is `last_reachable_at` (or, from an older hub, the
           last health sample): `last_pinged_at` moves on a failed probe too
           (review r13). The reason is the last probe's error. -->
      <HostOffline
        alias={host.alias}
        lastSeen={host.last_reachable_at ?? host.health_at ?? null}
        reason={host.last_probe_error ?? null}
        code={host.last_probe_error_code ?? null}
        {now}
        sessions={hostSessions.length}
        paused={hostSessions.map(sessionName)}
        onshow={hostSessions.length > 0 ? showSessions : null}
        ontry={onreprobe}
        trying={probing}
        tryBlocked={reprobeBlocked} />
    {/if}
    <KeyValue items={facts} testid="detail-facts" />
    <!-- ux F-14: disk, load, uptime and the agent version — the facts the
         live fleet had no signal for (two hosts at 98 % disk). -->
    <div class="health" data-testid="detail-health" aria-label="Health">
      {#if disk}
        <span class="disk" data-testid="detail-health-meter" data-level={disk.level}
          ><Meter value={disk.pct / 100} level={disk.level} label="disk used" /></span
        >
      {/if}
      <span class="line">{healthLine(host, now)}</span>
    </div>
    {#if attention}
      <p class="attention" data-testid="detail-attention"><Icon name={attention.icon} size={12} /> {attention.title}</p>
    {/if}
  </header>

  <div class="tabs" role="tablist" aria-label="{host.alias} sections" data-testid="detail-tabs">
    {#each [['overview', 'Overview', null], ['sessions', 'Sessions', hostSessions.length], ['lost', 'Lost & found', lostCount], ['provisioning', 'Provisioning', null]] as const as [id, label, count] (id)}
      <button
        type="button"
        role="tab"
        class="tab"
        aria-selected={tab === id}
        data-testid="detail-tab-{id}"
        onclick={() => (tab = id)}>{label}{#if count != null} <span class="muted">{count}</span>{/if}</button
      >
    {/each}
  </div>

  <!-- Orbit Fleet 4.7: health checklist, re-provision, new session here -->
  <section class="block" aria-label="Health checklist" data-testid="detail-checklist" hidden={tab !== 'overview'}>
    <div class="check-head">
      <h3>Health checklist</h3>
      <span class="muted" data-testid="detail-checked-at"
        >{lastCheck ? `checked ${formatAge(now - lastCheck.checked_at)} ago` : 'not checked yet'}</span
      >
      <span class="grow"></span>
      <button
        type="button"
        class="small"
        disabled={checking || checkBlocked !== null}
        title={checkBlocked ?? ''}
        data-testid="detail-run-checks"
        onclick={runChecks}>{checking ? 'checking…' : 'Run checks'}</button
      >
    </div>
    <ul class="checklist">
      {#each checklist as row (row.key)}
        <li data-testid="detail-check-row" data-key={row.key} data-state={row.state}>
          <span class="check-glyph" aria-hidden="true">{CHECK_GLYPH[row.state]}</span>
          <span class="check-label">{row.label}</span>
          <span class="check-detail">{row.detail}</span>
          {#if row.install}
            <span class="check-action">
              <AgentInstallAction alias={host.alias} version={hubVersion} testid="detail-agent-install" ondone={onreprobe} />
            </span>
          {:else if row.key === 'skills' && row.state === 'warn'}
            <span class="check-action">
              <button
                type="button"
                class="small"
                title="Open Assets and sync the drifted and missing skills"
                data-testid="detail-skills-sync"
                onclick={() => requestAssetsView({ command: 'sync' })}>Sync</button
              >
            </span>
          {/if}
        </li>
      {/each}
    </ul>
    {#if liveText}
      <div class="check-live" data-testid="detail-check-live">
        <Loader name="hex-field" size={96} testid="detail-check-loader" />
        <span class="check-live-step" data-testid="detail-check-live-step" aria-live="polite">{liveText}</span>
      </div>
    {/if}
    <div class="actions">
      <button
        type="button"
        class="action"
        class:advised={reprovisionAdvised}
        disabled={provisioning || provisionBlocked !== null}
        title={provisionBlocked ?? 'Write fleet’s hooks, skills and CLAUDE.md block to this host again'}
        data-testid="detail-reprovision"
        onclick={reprovision}>{provisioning ? 're-provisioning…' : 'Re-provision this host'}</button
      >
      {#if onnewsession}
        <button
          type="button"
          class="action"
          disabled={!host.reachable && !isLocal}
          data-testid="detail-new-session-here"
          onclick={onnewsession}><kbd>n</kbd> New session here</button
        >
        <button
          type="button"
          class="action"
          disabled={!host.reachable && !isLocal}
          title="A plain login shell on {host.alias}, in a project you pick"
          data-testid="detail-open-shell"
          onclick={() => openNewSessionPicker(host.alias, undefined, 'shell')}>Open a shell…</button
        >
      {/if}
    </div>
  </section>

  <!-- 2. Usage -->
  <section class="block" hidden={tab !== 'overview'}>
    {#if account}
      <div class="account-line" data-testid="detail-account">
        <span class="label">Account</span>
        <AccountNickname
          {account}
          editing={editingNickname}
          onedit={oneditstart}
          ondone={oneditdone}
          testid="detail-nickname"
          blocked={nicknameBlocked}
        />
        {#if account.email}<span class="muted">{account.email}</span>{/if}
      </div>
    {/if}
    {#if host.claude_profiles && host.claude_profiles.length > 0}
      <div class="account-line" data-testid="detail-profiles">
        <span class="label">Login profiles</span>
        <span class="profiles">
          {#each host.claude_profiles as p, i (p.name)}{#if i > 0}, {/if}<span
              class="profile"
              title="~/.claude-profiles/{p.name}"
            ><code>{p.name}</code> <span class="muted">{p.email ?? (p.account_uuid ? '' : 'not logged in')}</span></span>{/each}
        </span>
      </div>
    {/if}
    <!-- Usage is the embed page `embed.host_detail` (declarative pages L8). -->
    <EmbedSlot
      slot="host_detail"
      ctx={{
        now,
        locale,
        timeZone,
        host,
        account,
        snapshot,
        sharedWith,
        suppressUnavailable,
        onrefresh: onrefreshusage,
        refreshBlocked: refreshUsageBlocked,
      }}
    />
  </section>

  <!-- 3. Sessions -->
  <section class="block" aria-label="Sessions on {host.alias}" bind:this={sessionsBlock} hidden={tab !== 'sessions' && tab !== 'lost'}>
    <div class="section-head">
      <h3>{#if tab === 'lost'}Lost &amp; found <span class="muted">{lostCount}</span>{:else}Sessions <span class="muted">{hostSessions.length}</span>{/if}</h3>
      <div class="actions">
        {#if restorable.length > 0}
          <button
            type="button"
            class="small"
            disabled={busy || restoreBlocked !== null}
            title={restoreBlocked ??
              (restoreNotMine > 0
                ? `${restoreNotMine} of the ${restorable.length} lost sessions here belong to someone else and are left out`
                : '')}
            data-testid="restore-lost"
            onclick={onRestoreClick}
            >Restore {restorableMine.length || restorable.length} lost session{(restorableMine.length ||
              restorable.length) === 1
              ? ''
              : 's'}…{#if restoreNotMine > 0}<span class="muted"> ({restoreNotMine} not yours)</span>{/if}</button
          >
        {/if}
        {#if host.reachable}
          <button
            type="button"
            class="small"
            disabled={discoverBusy}
            data-testid="discover-lost"
            onclick={onDiscoverClick}
            >{discoverBusy ? 'searching…' : 'Find lost conversations…'}</button
          >
        {/if}
      </div>
    </div>
    <div class="tab-part" hidden={tab !== 'sessions'}>
    {#if tidyHint && onreviewtidy}
      <p class="muted" data-testid="detail-tidy-hint">
        {tidyHint.text}
        <button type="button" class="small" data-testid="detail-tidy-review" onclick={() => onreviewtidy(tidyHint.sessionIds)}
          >Review in Tidy</button
        >
      </p>
    {/if}
    {#if restoreError}
      <p class="error" data-testid="restore-error">{restoreError}</p>
    {/if}
    {#if restoreSummary}
      <p data-testid="restore-summary">
        Restored {restoreSummary.ok} of {restoreSummary.total}
        {#each restoreSummary.failures as f (f.name)}<br />{f.name}: {f.error}{/each}
      </p>
    {/if}
    </div>
    <div class="tab-part" hidden={tab !== 'lost'}>
    {#if lostCount === 0 && !discoverList && !discoverError}
      <p class="muted" data-testid="detail-lost-empty">Nothing found yet. Find lost conversations looks on {host.alias} for ones fleet could bring back.</p>
    {/if}
    {#if discoverError}
      <p class="error" data-testid="discover-error">{discoverError}</p>
    {/if}
    {#if discoverList}
      <div data-testid="discover-list">
        {#if findingFor}
          <p class="muted" data-testid="discover-finding-for">
            Other conversations {sessionName(findingFor)} could resume, its own project first.
          </p>
        {/if}
        {#if discoverList.length === 0}
          <p class="muted">No Claude conversations found on {host.alias}.</p>
        {:else}
          {#if resumeHubBlocked}
            <p class="muted" data-testid="discover-hub-note">Resume is unavailable right now: {resumeHubBlocked}</p>
          {/if}
          <ul class="discover-items">
            {#each shownCandidates ?? [] as c (c.claude_session_id)}
              <li class="discover-item">
                <div class="d-main">
                  <span class="d-cwd">{c.cwd}</span>
                  {#if c.git_branch}<span class="muted">{c.git_branch}</span>{/if}
                  <span class="muted">{shortAge(c.transcript_mtime, now)}</span>
                  {#if rankLabel(c.rank_hint)}<span class="badge">{rankLabel(c.rank_hint)}</span>{/if}
                  {#if c.derived_tmux_name}<span class="muted">{c.derived_tmux_name}</span>{/if}
                  {#if ignored.has(c.claude_session_id)}
                    <span class="badge" data-testid="discover-ignored">ignored</span>
                    <button type="button" class="small" data-testid="discover-unignore" onclick={() => setIgnored(c, false)}
                      >Bring back</button
                    >
                  {/if}
                </div>
                {#if c.existing_session_id !== null}
                  <span class="muted">already in fleet</span>
                {:else if resumedIds.has(c.claude_session_id)}
                  <span class="muted">resumed</span>
                {:else if c.resumable && c.project_id !== null && c.derived_tmux_name !== null && candidateBlocked(c)}
                  <span class="muted" title={candidateBlocked(c)} data-testid="discover-resume-blocked">resume unavailable</span>
                {:else if c.resumable && c.project_id !== null && c.derived_tmux_name !== null}
                  <button
                    type="button"
                    class="small"
                    data-testid="discover-resume"
                    disabled={resumingId === c.claude_session_id}
                    onclick={() => onResumeCandidate(c)}
                    >{resumingId === c.claude_session_id ? 'resuming…' : 'Resume'}</button
                  >
                  {#if resumeErrors[c.claude_session_id]}
                    <p class="error" data-testid="discover-item-error">{resumeErrors[c.claude_session_id]}</p>
                  {/if}
                {:else if needsRestoreInto(c)}
                  {#if restoringId === c.claude_session_id}
                    <LostTargetForm
                      action="Restore"
                      entry={c.git_branch ?? c.cwd}
                      requireProject
                      args={{
                        host_alias: host.alias,
                        claude_session_id: c.claude_session_id,
                        cwd: c.cwd,
                        git_branch: c.git_branch,
                      }}
                      onsubmit={(pid, ticket) => restoreInto(c, pid, ticket)}
                      oncancel={() => (restoringId = null)}
                      onignore={() => ignoreCandidate(c)}
                    />
                  {:else}
                    <button
                      type="button"
                      class="small"
                      disabled={candidateBlocked(c) !== null}
                      title={candidateBlocked(c) ?? 'Resume it in a project you pick'}
                      data-testid="discover-restore-into"
                      onclick={() => (restoringId = c.claude_session_id)}>Restore into…</button
                    >
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
          {#if ignoredCount > 0}
            <p class="muted" data-testid="discover-ignored-count">
              {ignoredCount} ignored on this device ·
              <button type="button" class="small" data-testid="discover-show-ignored" onclick={() => (showIgnored = !showIgnored)}
                >{showIgnored ? 'Hide them' : 'Show them'}</button
              >
            </p>
          {/if}
        {/if}
      </div>
    {/if}
    {#if outsidePanes.length > 0}
      <ul class="discover-items" data-testid="outside-panes" aria-label="Panes fleet did not start">
        {#each outsidePanes as p (p.id)}
          <li class="discover-item">
            <div class="d-main">
              <span class="d-cwd">{p.tmux_name}</span>
              <span class="badge">outside fleet</span>
              <span class="muted">running {shortAge(p.created_at, now)}</span>
            </div>
            {#if adoptingId === p.id}
              <LostTargetForm
                action="Adopt"
                entry={p.tmux_name}
                args={{ session_id: p.id }}
                onsubmit={(pid) => adoptInto(p, pid)}
                oncancel={() => (adoptingId = null)}
              />
            {:else}
              <button
                type="button"
                class="small"
                disabled={adoptBlocked(p) !== null}
                title={adoptBlocked(p) ?? 'Fleet runs it from now on; the pane stays as it is'}
                data-testid="outside-adopt"
                onclick={() => (adoptingId = p.id)}>Adopt…</button
              >
              {#if paneIgnored.has(p.tmux_name)}
                <button type="button" class="small" data-testid="outside-unignore" onclick={() => setPaneIgnoredHere(p, false)}
                  >Bring back</button
                >
              {:else}
                <button
                  type="button"
                  class="small"
                  title="Leave it out of this list on this device; the pane keeps running"
                  data-testid="outside-ignore"
                  onclick={() => ignorePane(p)}>Ignore</button
                >
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if ignoredPaneCount > 0}
      <p class="muted" data-testid="outside-ignored-count">
        {ignoredPaneCount} ignored on this device ·
        <button
          type="button"
          class="small"
          data-testid="outside-show-ignored"
          onclick={() => (showIgnoredPanes = !showIgnoredPanes)}>{showIgnoredPanes ? 'Hide them' : 'Show them'}</button
        >
      </p>
    {/if}
    </div>
    <div class="tab-part" hidden={tab !== 'sessions'}>
    {#if hostSessions.length === 0}
      <p class="muted">No sessions on this host. Press <kbd>n</kbd> to start one.</p>
    {:else}
      <ul class="sessions">
        {#each hostSessions as s (s.id)}
          <li>
            <button
              type="button"
              class="session"
              data-nav-row
              data-testid="detail-session"
              onclick={() => selectSessionExplicitly(s)}
            >
              <span class="s-name">{sessionName(s)}</span>
              <span class="muted">{sessionState(s)}</span>
            </button>
            {#if host.reachable && isRestorable(s)}
              <button
                type="button"
                class="small"
                disabled={discoverBusy}
                title="Its own conversation is gone or wrong? Pick another one found on this host"
                data-testid="detail-find-another"
                onclick={() => findAnother(s)}>Find another…</button
              >
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    </div>
  </section>

  <!-- 5. Integration -->
  <section class="block" aria-label="Integration" hidden={tab !== 'provisioning'}>
    <h3>Integration</h3>
    <div class="kv">
      <span
        class="label"
        title="Which harnesses the asset catalog syncs here. auto = Codex where the codex CLI, ~/.codex/auth.json or ~/.codex/sessions is found; off = the next sync removes what fleet installed for Codex"
        >Codex</span
      >
      <select
        value={codexValue}
        disabled={busy || harnessBlocked !== null}
        title={harnessBlocked ?? ''}
        aria-label="Codex assets"
        data-testid="detail-codex"
        onchange={(e) => (codexDraft = { alias: host.alias, mode: (e.currentTarget as HTMLSelectElement).value as HarnessMode })}
      >
        <option value="auto">auto</option>
        <option value="on">on</option>
        <option value="off">off</option>
      </select>
    </div>
    <div class="kv">
      <span class="label" title="Control-API token: full = every tool, readonly = observe only">Token</span>
      {#if token}
        <select
          value={tokenValue}
          disabled={busy || adminBlocked !== null}
          title={adminBlocked ?? ''}
          aria-label="Token mode"
          data-testid="detail-token-mode"
          onchange={(e) => (tokenDraft = { alias: host.alias, mode: (e.currentTarget as HTMLSelectElement).value as TokenMode })}
        >
          <option value="full">full</option>
          <option value="readonly">readonly</option>
        </select>
      {:else}
        <span class="muted" data-testid="detail-token-empty"
          >{hostTokensBlocked ?? (tokensLoaded ? 'none — provision hosts to mint one' : '…')}</span
        >
      {/if}
    </div>
    <div class="kv">
      <span class="label" title="Installed with the host's token; last event = newest Stop hook from a session on this host">Hooks</span>
      <span data-testid="detail-hooks" data-state={hook.state}>{hookHealthLabel(hook, now)}</span>
    </div>
    <div class="actions" data-testid="detail-integrations-bar">
      <button
        type="button"
        class="small"
        disabled={busy || !integrationDirty}
        data-testid="detail-integrations-discard"
        onclick={discardIntegrations}>Discard</button
      >
      <button
        type="button"
        class="small primary"
        disabled={busy || !integrationDirty}
        data-testid="detail-integrations-save"
        onclick={() => void saveIntegrations()}>Save</button
      >
    </div>
    {#if token}
      <button
        type="button"
        class="action"
        disabled={busy || adminBlocked !== null}
        title={adminBlocked ?? ''}
        data-testid="detail-rotate"
        onclick={() => (confirm = 'rotate')}>Rotate token…</button
      >
    {/if}
  </section>

  <!-- 6. Danger -->
  <section class="block danger-zone" aria-label="Danger" hidden={tab !== 'provisioning'}>
    <h3>Danger</h3>
    {#if isLocal}
      <p class="muted">The local host can't be hidden or removed.</p>
    {:else}
      <div class="actions">
        <button
          type="button"
          class="action"
          disabled={busy || adminBlocked !== null}
          title={adminBlocked ?? ''}
          data-testid="detail-hide"
          onclick={onHideToggle}>{host.hidden ? 'Show host' : 'Hide host'}</button
        >
        <button
          type="button"
          class="action danger"
          disabled={busy || adminBlocked !== null}
          title={adminBlocked ?? ''}
          data-testid="detail-remove"
          onclick={() => (confirm = 'remove')}>Remove host…</button
        >
      </div>
    {/if}
  </section>
</section>

{#if confirm === 'rotate'}
  <ConfirmDialog
    title="Rotate the token for {host.alias}?"
    message={rotateTokenMessage(host.alias)}
    confirmLabel="Rotate token"
    danger
    {busy}
    confirmTestId="confirm-rotate"
    onconfirm={confirmRotate}
    oncancel={() => (confirm = null)}
  />
{:else if confirm === 'remove'}
  <ConfirmDialog
    title="Remove {host.alias}?"
    message={removeHostMessage(host.alias, hostSessions.length)}
    confirmLabel="Remove host"
    danger
    {busy}
    confirmTestId="confirm-remove"
    onconfirm={confirmRemove}
    oncancel={() => (confirm = null)}
  />
{:else if confirm === 'restore'}
  <ConfirmDialog
    title="Restore lost sessions on {host.alias}?"
    confirmLabel="Restore"
    {busy}
    confirmDisabled={restoreCount === 0}
    confirmTestId="confirm-restore"
    onconfirm={confirmRestore}
    oncancel={cancelRestore}
  >
    <ul class="restore-plan">
      {#each restorePlan ?? [] as entry (entry.session_id)}
        <li>
          <span class="name">{entry.friendly_name ?? entry.tmux_name}</span>
          {#if entry.cwd}<span class="muted">{entry.cwd}</span>{/if}
          {#if entry.action === 'skip'}<span class="skip">skipped — {entry.reason}</span>
          {:else if !restorableMineIds.has(entry.session_id)}<span class="skip"
              >left alone — not yours</span
            >{/if}
        </li>
      {/each}
    </ul>
    {#if restorePlanNotMine > 0}
      <p class="note" data-testid="restore-not-mine">
        {restorePlanNotMine} of these belong to someone else and will be left alone — only the
        session's owner can restore it.
      </p>
    {/if}
    {#if restoreCount === 0}
      <p class="note" data-testid="restore-nothing">Nothing here can be restored.</p>
    {:else}
      <p class="note">Each session resumes its Claude conversation. Any first-run prompt waits for you.</p>
    {/if}
  </ConfirmDialog>
{/if}

<style>
  .host-detail {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 0.8rem 1rem 2rem;
    overflow-y: auto;
    min-height: 0;
    height: 100%;
    box-sizing: border-box;
    outline: none;
    font-size: var(--text-2xs);
  }
  .title-row { display: flex; align-items: baseline; gap: 0.6rem; flex-wrap: wrap; }
  h2 { margin: 0; font-size: var(--text-lg); }
  h3 {
    margin: 0 0 0.35rem;
    font-size: var(--text-2xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-muted);
  }
  .status.off { color: var(--usage-warn); }
  .muted { color: var(--fg-muted); }
  .health { display: flex; align-items: center; gap: 0.5rem; margin-top: 0.25rem; font-size: var(--text-2xs); }
  .disk { width: 6rem; flex-shrink: 0; }
  .attention { margin: 0.4rem 0 0; color: var(--usage-warn); }
  .block { border-top: 1px solid var(--border); padding-top: 0.6rem; }
  .block[hidden], .tab-part[hidden] { display: none; }
  .tabs { display: flex; gap: 0.25rem; border-bottom: 1px solid var(--border); }
  .tab {
    padding: 0.35rem 0.6rem;
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .tab[aria-selected='true'] { color: var(--fg); border-bottom-color: var(--accent); }
  .account-line { display: flex; align-items: baseline; gap: 0.5rem; margin-bottom: 0.4rem; min-width: 0; }
  .label { color: var(--fg-muted); min-width: 3.5rem; }
  .section-head { display: flex; align-items: baseline; justify-content: space-between; gap: 0.6rem; flex-wrap: wrap; }
  .section-head h3 { margin: 0; }
  .error { color: var(--usage-crit); margin: 0.4rem 0; }
  .restore-plan { list-style: none; margin: 0.4rem 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  .restore-plan li { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .restore-plan .skip { color: var(--usage-warn); }
  .note { margin: 0.4rem 0 0; color: var(--fg-muted); }
  .discover-items { list-style: none; margin: 0.4rem 0 0; padding: 0; display: flex; flex-direction: column; gap: 0.4rem; }
  .discover-item {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
    padding: 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .d-main { display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem; flex: 1; min-width: 0; }
  .d-cwd { font-variant-numeric: tabular-nums; }
  .badge {
    font-size: var(--text-2xs);
    padding: 0 0.35rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
    color: var(--fg-muted);
  }
  .sessions { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .sessions li { display: flex; align-items: center; gap: 4px; }
  .session {
    display: flex;
    gap: 0.6rem;
    width: 100%;
    padding: 0.2rem 0.3rem;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .session:hover,
  .session:focus-visible { border-color: var(--border); background: color-mix(in srgb, var(--fg) 5%, transparent); }
  .session:focus-visible { outline: var(--ring-w) solid var(--ring); outline-offset: calc(-1 * var(--ring-w)); }
  .s-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .kv { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.3rem; }
  select {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: var(--radius-sm);
    font-size: var(--text-2xs);
  }
  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .action,
  .small {
    font-size: var(--text-2xs);
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .action:disabled,
  .small:disabled { opacity: 0.55; cursor: default; }
  .action.danger { color: var(--usage-crit); border-color: var(--usage-crit); }
  kbd {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    padding: 0 0.2rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-xs);
  }
  .check-head { display: flex; align-items: baseline; gap: 0.5rem; }
  .check-head h3 { margin: 0; }
  .grow { flex: 1; }
  .check-live { display: flex; align-items: center; gap: 0.75rem; margin: 0.4rem 0; }
  .check-live-step { font-size: var(--text-2xs); color: var(--fg-muted); }
  .checklist { list-style: none; margin: 0.4rem 0; padding: 0; font-size: var(--text-2xs); }
  .checklist li { display: grid; grid-template-columns: 1.2rem 9rem 1fr; gap: 0.4rem; padding: 0.15rem 0; }
  .checklist .check-action { grid-column: 3; }
  .checklist li[data-state='ok'] .check-glyph { color: var(--usage-ok); }
  .checklist li[data-state='warn'] .check-glyph,
  .checklist li[data-state='warn'] .check-detail { color: var(--usage-warn); }
  .checklist li[data-state='fail'] .check-glyph,
  .checklist li[data-state='fail'] .check-detail { color: var(--usage-crit); }
  .checklist li[data-state='unknown'] .check-detail,
  .checklist li[data-state='na'] .check-detail { color: var(--fg-muted); }
  .action.advised { border-color: var(--usage-warn); }
</style>
