<script lang="ts">
  import { applyWorkEvents, loadTrackers, sessionsMentioning } from './lib/trackers';
  import { loadOrgs, cycleScope } from './lib/orgs';
  import type { WorkEvent } from './lib/trackers';
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import { appVersion, loadAppVersion, versionLine } from './lib/app_version';
  import { setContextRedPct } from './lib/attention';
  import { trackersHealth, trackersSummary } from './lib/tracker_health';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import SessionTabs, { type SessionTab } from './lib/SessionTabs.svelte';
  import { openInEditorIfAllowed } from './lib/editor';
  import { todayOpen } from './lib/today';
  import {
    bumpWorkChanged,
    cycleWorkOrg,
    noteWorkChanged,
    noteWorkEvents,
    sessionEventsTouchWork,
    selectedTaskId,
    sidebarView,
    taskDetailOpen,
    toggleSidebarView,
  } from './lib/work_view';
  import type { SessionEvent } from './lib/sessions';
  import { composerInsert } from './lib/conversation';
  import { tidyRequest } from './lib/tidy';
  import TerminalView from './lib/TerminalView.svelte';
  import { terminalPane, requestTerminalTab } from './lib/terminals';
  import WatchView from './lib/WatchView.svelte';
  import FilesPanel from './lib/FilesPanel.svelte';
  import HostsView from './lib/HostsView.svelte';
  import ConversationPanel from './lib/ConversationPanel.svelte';
  import AssetsPanel from './lib/AssetsPanel.svelte';
  import Toolkit from './lib/Toolkit.svelte';
  import { toolkitTab } from './lib/toolkit_skills';
  import AccountsPage from './lib/AccountsPage.svelte';
  import AppRail from './lib/AppRail.svelte';
  import ControlView from './lib/ControlView.svelte';
  import AutomationView from './lib/AutomationView.svelte';
  import { toggleControl, toggleToday } from './lib/control';
  import type { RailId } from './lib/rail';
  import WorkBoard from './lib/WorkBoard.svelte';
  import { loadProjects, applyProjectEvents } from './lib/projects';
  import { loadSessions, applySessionEvents, sessions, sessionsAnswered, hasNoPane, showFriendlyNames, sidebarGroupBy } from './lib/sessions';
  import { bootstrapError as bootstrapFailure } from './lib/bootstrap_state';
  import { errorText } from './lib/error_copy';
  import { loadHosts, applyHostEvents, hosts } from './lib/hosts';
  import { viewHostSessions } from './lib/host_actions';
  import { loadAccounts, applyAccountEvents, accounts } from './lib/accounts';
  import { loadTasks, applyTaskEvents } from './lib/tasks';
  import { loadAccountUsage, applyAccountUsageEvents, accountUsage } from './lib/account_usage_store';
  import EmbedSlot from './lib/pages/EmbedSlot.svelte';
  import { mergeInventoryRow, clearInventoryFor, loadAssets, primeCatalog, syncProgress, repoStatus } from './lib/assets';
  import { subscribeToRowEvents } from './lib/events';
  import { startVoiceEvents } from './lib/voice';
  import { accessOf, applyGrantChanges, loadMyGrants } from './lib/access';
  import TransferSheet from './lib/TransferSheet.svelte';
  import ShareSheet from './lib/ShareSheet.svelte';
  import { applyMoveProgress, recheckWaitingRuns } from './lib/moves';
  import { dispatchTimelineEvents, dispatchConversationsChanged } from './lib/live_events';
  import Toasts from './lib/Toasts.svelte';
  import QuickSwitcher from './lib/QuickSwitcher.svelte';
  import ShortcutSheet from './lib/ShortcutSheet.svelte';
  import NewSessionDialog from './lib/NewSessionDialog.svelte';
  import { newSessionRequest, clearNewSessionRequest } from './lib/new_session_request';
  import { push, pushError } from './lib/toasts';
  import DownloadsSheet from './lib/DownloadsSheet.svelte';
  import { downloads, downloadsOpen, unseen, loadDownloads, noteDownloadsChanged } from './lib/downloads';
  import { loadLocalWorkspaces, noteLocalWorkspacesChanged } from './lib/local_workspaces';
  import type { Result } from './lib/result';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { selectedSession, restoreLastSession, selectSessionExplicitly, onSessionOpened } from './lib/selection';
  import {
    addProjectRequest,
    appChord,
    assetsViewRequest,
    hostsChordLabel,
    hostsViewRequest,
    onHostsCloseRequested,
    openPathRequest,
    requestNewSessionOnHost,
    sessionViewChordLabel,
    settingsOpen,
    openSettingsAt,
    shortcutSheetOpen,
  } from './lib/app_views';
  import { detectMac, isEditable } from './lib/terminal_keys';
  import { loadSessionUi, saveSessionUi, DEFAULT_UI } from './lib/session_ui';
  import { readPref, writePref, sessionView, uiLayout } from './lib/prefs';
  import { sessionActionRequest } from './lib/session_actions';
  import { resolveSessionView, otherSessionView, type SessionView } from './lib/session_view';
  import WelcomeDialog from './lib/WelcomeDialog.svelte';
  import HintLayer from './lib/HintLayer.svelte';
  import McpConfirmDialog from './lib/McpConfirmDialog.svelte';
  import AgentFab from './lib/AgentFab.svelte';
  import AgentPanel from './lib/AgentPanel.svelte';
  import { agentPanelOpen, closeAgent, toggleAgent } from './lib/operator';
  import type { AgentContextInput } from './lib/agent_context';
  import { onboardingWelcomed, onboardingDismissed } from './lib/onboarding';
  import { loadComposerPresets, refreshComposerPresetsIfIdle } from './lib/composer_presets';
  import { hubStatus, loadHubStatus } from './lib/hub';
  import HubUnavailableBanner from './lib/HubUnavailableBanner.svelte';
  import { startHubConnection, setGapHandler, hubConnection } from './lib/hub_connection';
  import StatusBarMark from './lib/StatusBarMark.svelte';
  import ShellHeader from './lib/ShellHeader.svelte';
  import StartupSplash from './lib/StartupSplash.svelte';
  import UpdateReveal from './lib/UpdateReveal.svelte';
  import { markStartup, startCatchUp, takeUpdateReveal, trackActivity } from './lib/startup';
  import { loadProjectPicks } from './lib/project_picks';
  import HubConnectionBanner from './lib/HubConnectionBanner.svelte';
  import { get } from 'svelte/store';
  import { destination, goTo, leave } from './lib/destination';

  const isNumber = (v: unknown): v is number => typeof v === 'number';
  const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

  // Sidebar width is global. Sidebar collapsed state is also global — unlike
  // the center pane (which the user wants per-session), the sidebar is the
  // project tree itself and doesn't make sense to differ between sessions.
  let sidebarPx = $state(readPref('layout.sidebar', 280, isNumber));
  let sidebarCollapsed = $state(readPref('layout.sidebar-collapsed', false, isBool));
  // sidebarPx changes on every resize-drag frame; debounce the localStorage
  // write so a drag persists once (on settle) instead of per frame.
  let sidebarSaveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const px = sidebarPx;
    clearTimeout(sidebarSaveTimer);
    sidebarSaveTimer = setTimeout(() => writePref('layout.sidebar', px), 200);
    return () => clearTimeout(sidebarSaveTimer);
  });
  $effect(() => {
    writePref('layout.sidebar-collapsed', sidebarCollapsed);
  });

  // Center pane WIDTH is per-session — the user said "Kazda session ma mat
  // aj vlastnu pamat UI, nastavenia rozdelenia". When the user picks a
  // session we hydrate centerPx from localStorage; when they resize we
  // persist it back under that session's key (below).
  let centerPx = $state(DEFAULT_UI.centerPx);
  // Center COLLAPSED state is GLOBAL (like the sidebar) — the user wants the
  // pane to remember collapsed/expanded across restarts regardless of which
  // session is open, so it lives in prefs.ts, not the per-session record.
  let centerCollapsed = $state(readPref('layout.center-collapsed', false, isBool));
  $effect(() => {
    writePref('layout.center-collapsed', centerCollapsed);
  });

  // When the selected session changes, swap in its persisted layout. The
  // save-effect below is gated on a per-session token (`hydratedKey`) rather
  // than a boolean+microtask: a boolean races across rapid session switches
  // and can save one session's centerPx under another's key.
  let hydratedKey: string | null = null;
  const sessionKey = (s: { host_alias: string; tmux_name: string }) =>
    `${s.host_alias}/${s.tmux_name}`;
  // `$selectedSession` is derived from the sessions store, so its object
  // identity changes on every `session:updated` (each reconcile tick). Key
  // the layout effects on the stable host/name string so they don't re-run
  // — and re-arm the save timer — for updates that don't change which
  // session is open; the row itself is read untracked inside.
  const selectedKey = $derived($selectedSession ? sessionKey($selectedSession) : null);
  $effect(() => {
    const key = selectedKey;
    if (!key) return;
    if (key === hydratedKey) return;
    const sess = untrack(() => $selectedSession);
    if (!sess) return;
    const ui = loadSessionUi(sess.host_alias, sess.tmux_name);
    centerPx = ui.centerPx;
    hydratedKey = key;
  });

  // centerPx changes per resize-drag frame too — debounce its persistence.
  let centerSaveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const key = selectedKey;
    const px = centerPx;
    if (!key) return;
    // Only persist once centerPx has actually been hydrated FOR this session
    // — otherwise we'd write the previous session's value under this key.
    if (key !== hydratedKey) return;
    const sess = untrack(() => $selectedSession);
    if (!sess) return;
    const { host_alias, tmux_name } = sess;
    clearTimeout(centerSaveTimer);
    centerSaveTimer = setTimeout(() => saveSessionUi(host_alias, tmux_name, { centerPx: px }), 200);
    return () => clearTimeout(centerSaveTimer);
  });

  let health = $state<Health | null>(null);
  // Which version belongs to whom. `health` is the FLEET in front of the
  // reader — this app's own numbers standalone, the hub's over the wire —
  // so the footer labels it rather than leaving one `v…` to mean either.
  const versions = $derived(
    versionLine({
      app: $appVersion,
      health,
      remote: $hubStatus.remote,
      hubUrl: $hubStatus.url,
    }),
  );
  const trackersLine = $derived(trackersSummary($trackersHealth));
  let healthError = $state<string | null>(null);
  /** Review r13: the health failure in a sentence; the code stays under Details. */
  let healthText = $state<string | null>(null);
  // Bootstrap (initial list_* fetches) failures. These used to be swallowed,
  // so a broken DB showed an innocent "No projects yet". Now they surface as
  // a sticky error toast (with the E_* code) plus this footer banner.
  let bootstrapError = $state<string | null>(null);
  /** Review r13: which lists failed, in words ("sessions, hosts"). */
  let bootstrapWhat = $state<string | null>(null);
  let bootstrapRetrying = $state(false);

  /** Review r13: Retry on the footer's failed-startup line re-reads the
   *  lists that failed; the line goes when they all answer. */
  async function retryBootstrap() {
    bootstrapRetrying = true;
    const [pr, sr, hr, ar] = await Promise.all([loadProjects(), loadSessions({ force: true }), loadHosts(), loadAccounts()]);
    bootstrapRetrying = false;
    noteBootstrap(pr, sr, hr, ar);
  }

  function noteBootstrap(pr: Result<unknown>, sr: Result<unknown>, hr: Result<unknown>, ar: Result<unknown>) {
    const lists: [string, Result<unknown>][] = [['projects', pr], ['sessions', sr], ['hosts', hr], ['accounts', ar]];
    const failed = lists.filter(([, r]) => !r.ok);
    bootstrapWhat = failed.length > 0 ? failed.map(([what]) => what).join(', ') : null;
    if (failed.length === 0) bootstrapError = null;
    // The Sessions list and the Inbox read this, so a failed load is never
    // shown as "No projects yet" or "Nothing needs you".
    // A skewed hub (`E_HUB_CONTRACT`) has its own sentence in the list,
    // from the connection banner.
    const listFailure = !sr.ok ? sr.error : !pr.ok ? pr.error : null;
    bootstrapFailure.set(listFailure && listFailure.code !== 'E_HUB_CONTRACT' ? listFailure : null);
  }
  let unlistenEvents: UnlistenFn | null = null;
  let unlistenVoice: UnlistenFn | null = null;
  let showWelcome = $state(false);
  // Redesign step 3.15: the version to reveal once after an update.
  let revealVersion = $state<string | null>(null);
  let stopActivity: (() => void) | null = null;
  let stopCatchUp: (() => void) | null = null;
  // File downloads: the footer button and its sheet.
  const unseenDownloads = $derived(unseen($downloads));

  function reportBootstrap(what: string, r: Result<unknown>): string | null {
    if (r.ok) return null;
    // `E_HUB_CONTRACT` is the one code every bootstrap load can fail with at
    // once (a skewed hub refuses all of them the same way), and it already
    // has two permanent homes on screen: the hub-connection banner at the
    // top of the window and this footer, right below, naming which loads
    // failed. Stacking up to four more sticky toasts on top of that would
    // just be the #166 "toast storm" repeated — so this is the one code that
    // does not also toast. Every other failure still does.
    if (r.error.code !== 'E_HUB_CONTRACT') pushError(r.error, `Failed to load ${what}`);
    return `${what}: ${r.error.code}`;
  }

  let trackerRefresh: ReturnType<typeof setInterval> | null = null;
  onDestroy(() => {
    if (trackerRefresh) clearInterval(trackerRefresh);
    stopActivity?.();
    stopCatchUp?.();
  });

  // `work:*` frames: trackers and their first sync. When a tracker finishes
  // its FIRST sync, the sessions whose keys it owns just got titles and
  // status (retro-binding); say how many, and offer the Group-by-Work view.
  function onWorkEvents(events: WorkEvent[]) {
    // The Work view (M14) re-reads what it shows when an item moved.
    noteWorkEvents(events);
    for (const t of applyWorkEvents(events)) {
      const keys = get(sessions).map((s) => s.work?.key ?? null);
      const { count, prefixes } = sessionsMentioning(t, keys);
      if (count === 0) continue;
      push({
        kind: 'info',
        message: `${count} session${count === 1 ? '' : 's'} mention ${prefixes.map((p) => `${p}-*`).join(', ')}`,
        action: {
          label: 'Review',
          run: () => {
            sidebarView.set('sessions');
            sidebarGroupBy.set('work');
          },
        },
      });
    }
  }

  // Session events that move a session's work (or the attention of one the
  // Work view shows) refresh the Work view too; compared before the store
  // takes them.
  function onSessionEvents(events: SessionEvent[]) {
    if (sessionEventsTouchWork(events)) bumpWorkChanged('session');
    applySessionEvents(events);
  }

  onMount(async () => {
    // Before anything is awaited: whether this launch is warm (no splash)
    // is decided from the stamp the last run left.
    stopActivity = trackActivity();
    // FIRST, and awaited: the rest of this function branches on it. A hub
    // client must not poll account usage (the backend refuses it, so it would
    // be an error toast on every launch for a panel that does not apply), and
    // the footer names the hub it is a window onto.
    await loadHubStatus();
    // Not awaited and never fatal: the version is for the footer and for a
    // bug report, and `versionLine` says less rather than guessing when it
    // is missing. Before the `unavailable` return below, so a window that
    // reaches no hub at all can still say which app it is.
    void loadAppVersion().then(() => (revealVersion = takeUpdateReveal(get(appVersion))));
    // A hub is configured but this launch could not use it. The backend owns
    // nothing and refuses every fleet command, so each load below would only
    // add an error toast under the banner that already explains all of them.
    // The window shows that banner and the way to Settings, and nothing else.
    if (get(hubStatus).unavailable) {
      markStartup('done');
      // Review r13: no list will arrive, so nothing waits on one (⌘K's
      // "still arriving", the first-load loaders).
      sessionsAnswered.set(true);
      return;
    }
    // Only a hub client has a live link to lose; see HubConnectionBanner.
    if (get(hubStatus).remote) void startHubConnection();
    const hr0 = await healthCheck();
    // `health_check` routes to the hub's `fleet_health` in remote mode, so
    // it hits the same skewed-contract gate as every list load below — and
    // gets the same treatment: no toast (the banner already says it), and
    // its failure folds into the one footer line below instead of the
    // generic "health check failed" wording, which would both toast on its
    // own and hide which loads actually failed. Every other failure code
    // keeps today's behaviour: it sets `healthError` (which pre-empts the
    // footer's bootstrap line — a real health failure is the more important
    // thing to say) and toasts.
    let healthFailure: string | null = null;
    if (hr0.ok) {
      health = hr0.value;
      setContextRedPct(hr0.value.context_red_pct);
      trackersHealth.set(hr0.value.trackers ?? null);
    } else if (hr0.error.code === 'E_HUB_CONTRACT') {
      healthFailure = `health: ${hr0.error.code}`;
    } else {
      healthError = `${hr0.error.code}: ${hr0.error.message}`;
      healthText = errorText(hr0.error);
      push({ kind: 'error', code: hr0.error.code, message: `Health check failed: ${hr0.error.message}` });
    }
    // Subscribed BEFORE the first list: a `session:updated` that lands while
    // the list is in flight would otherwise be emitted to no listener and
    // lost until the row changes again.
    unlistenVoice = await startVoiceEvents();
    unlistenEvents = await subscribeToRowEvents({
      onSessionEvents: onSessionEvents,
      onHostEvents: applyHostEvents,
      onAccountEvents: applyAccountEvents,
      onProjectEvents: applyProjectEvents,
      onTaskEvents: applyTaskEvents,
      onAccountUsageEvents: applyAccountUsageEvents,
      onTimelineEvents: dispatchTimelineEvents,
      onConversationsChanged: dispatchConversationsChanged,
      onAssetInventoryUpdated: mergeInventoryRow,
      onAssetInventoryCleared: (p) => clearInventoryFor(p.host_alias, p.harness),
      onCatalogLoaded: () => { void loadAssets(); void repoStatus(); },
      onSyncProgress: (p) => syncProgress.set(p),
      onMoveProgress: applyMoveProgress,
      onWorkEvents: onWorkEvents,
      // `work:changed` (M14): the Work view re-reads.
      onWorkChanged: noteWorkChanged,
      // File downloads: ids only, so the list is re-read.
      onDownloadsChanged: noteDownloadsChanged,
      // Local workspace sync: ids only, so the list is re-read.
      onLocalWorkspacesChanged: noteLocalWorkspacesChanged,
      // `grant:changed` (M1): a share or a revoke moves no column on any row,
      // so this is the only thing that tells a client its own grant set
      // changed. It patches `access.ts`, and everything derived from it — the
      // terminal gate included — re-evaluates without a re-list. A revoke
      // closing an attached PTY depends on this frame arriving.
      onGrantChanged: applyGrantChanges,
    });
    markStartup('backend');
    const [pr, sr, hr, ar] = await Promise.all([
      loadProjects(),
      // Each marks its startup stage as it lands (step 3.15).
      loadSessions().then((r) => {
        markStartup('sessions');
        return r;
      }),
      loadHosts().then((r) => {
        markStartup('hosts');
        return r;
      }),
      loadAccounts(),
      // This client's own person id and grant set (multi-user M1). Awaited
      // with the lists because `restoreLastSession()` below selects a row and
      // the terminal gate reads the answer the moment it does — a paired
      // desktop that learned its identity a beat later would flash "no
      // terminal" over the owner's own session.
      //
      // Deliberately NOT in the `failures` list: a standalone desktop needs no
      // answer at all (it owns the fleet, so every row it holds is its own),
      // and a backend older than the command has none to give — turning either
      // into a startup error would report a problem that changes nothing.
      loadMyGrants(),
    ]);
    // The picker's pins and groups. Outside the `Promise.all` on purpose: a
    // hub older than the feature has no answer, and that is not a startup
    // failure (the picker then runs on its rules alone).
    void loadProjectPicks();
    const failures = [
      healthFailure,
      reportBootstrap('projects', pr),
      reportBootstrap('sessions', sr),
      reportBootstrap('hosts', hr),
      reportBootstrap('accounts', ar),
    ].filter((f): f is string => f !== null);
    if (failures.length > 0) bootstrapError = `startup load failed — ${failures.join(', ')}`;
    noteBootstrap(pr, sr, hr, ar);
    markStartup('done');
    stopCatchUp = startCatchUp(get(hosts));
    // Sessions are loaded now — re-open the one the user last had selected.
    // Only when the list actually arrived: on a failed fetch the store is
    // empty, and restoreLastSession() would take that as "the session is
    // gone" and erase the persisted pref — losing the selection over a
    // transient DB error.
    if (sr.ok) restoreLastSession();
    // First-run welcome: only when never shown AND the fleet is empty.
    const visibleHostCount = get(hosts).filter((h) => !h.hidden).length;
    const workSessionCount = get(sessions).filter((s) => !hasNoPane(s)).length;
    if (!get(onboardingWelcomed) && visibleHostCount === 0 && workSessionCount === 0) {
      showWelcome = true;
    }
    // Tasks are secondary to the session list: load after the row
    // subscription is live so no `task:updated` is missed, and never block
    // startup on it (a failure only leaves the Tasks panel empty).
    void loadTasks();
    // Trackers (work graph M3): their state badges, chip staleness and the
    // quick switcher's tickets. A hub older than M3 has no answer.
    void loadTrackers();
    // The asset catalog, for the quick switcher's asset rows: only the Assets
    // panel filled it, so ⌘K listed none on a fresh launch. Silent.
    void primeCatalog(get(hubStatus).remote);
    // A hub reconnect the hub could not replay: the backend re-lists rows
    // itself; projects/worktrees and trackers/work have list shapes their
    // events cannot carry, so this window re-fetches them here. The chips
    // have no event at all, so they are re-read too (unless an edit is
    // pending: a reload must not replace a half-typed chip).
    // File downloads: the footer's count (a hub older than revision 7 is
    // refused before this, so a failure is just an empty list).
    void loadDownloads();
    // Local workspace links live in this machine's database (empty when
    // paired with a hub), so no gap can make them stale.
    void loadLocalWorkspaces();
    setGapHandler(() => {
      void loadDownloads();
      void loadProjects();
      void loadTrackers();
      void refreshComposerPresetsIfIdle();
      // A gap the hub could not replay can have swallowed a `grant:changed`,
      // and a grant set that quietly lost an entry is a shared session that
      // has vanished from reach (or, worse, a revoked one still reachable).
      // Re-read it rather than trusting the patched copy.
      void loadMyGrants();
    });
    // The composer's chip row. Fleet state since it moved off `localStorage`
    // (so the phone and this window share one list), and never on the
    // critical path: the cached copy is already on screen, and a failed read
    // leaves those chips up rather than an error toast about buttons.
    void loadComposerPresets();
    // Orgs (work graph M5): the scope selector, colour bars and Settings.
    // Org changes arrive as `session:updated` for the rows they move; the
    // list itself is refreshed with the trackers.
    void loadOrgs();
    // A tracker's `last_sync_at` moves every pass without a frame (the sync
    // pushes only real changes), so the chips' "synced … ago" and stale
    // clock read a copy refreshed here.
    trackerRefresh = setInterval(() => {
      void loadTrackers();
      void loadOrgs();
    }, 120_000);
    // Account usage: same reasoning — not on the critical bootstrap path,
    // loaded after the subscription so no `account_usage:updated` is missed.
    //
    // Not while a hub owns the fleet: this app runs no usage poller then, so
    // `list_account_usage` is guarded on the backend and answers
    // `E_LOCAL_ONLY`. Calling it anyway would put an error toast on every
    // launch about a panel that simply does not apply here.
    if (!get(hubStatus).remote) void loadAccountUsage();
  });

  // Catch-up net for missed Tauri events (e.g. sleep/wake, dropped events).
  // With M3 events flowing, the store stays fresh by itself most of the time —
  // throttle the focus-driven re-fetch to 30s so alt-tabbing doesn't hammer
  // the backend with a full list_projects + list_sessions on every focus.
  let lastFocusFetch = 0;
  const FOCUS_FETCH_INTERVAL_MS = 30_000;
  function onFocus() {
    // Refused while the configured hub cannot be used; see onMount.
    if (get(hubStatus).unavailable) return;
    const now = Date.now();
    if (now - lastFocusFetch < FOCUS_FETCH_INTERVAL_MS) return;
    lastFocusFetch = now;
    // Both discard their Result, same as every other focus-driven refresh:
    // the next event or refresh heals a transient failure. A contract skew
    // (`E_HUB_CONTRACT`) will not heal on its own, but it is not silent
    // either — the hub-connection banner already says so, persistently, so a
    // toast on every alt-tab back into the window would only repeat that.
    void loadProjects();
    void loadSessions();
    // A Transfer waiting for its session to go idle hears of the wait's end
    // only through the live timeline push; one missed while the window was
    // away (sleep, a dropped stream) is read back from the timeline here.
    void recheckWaitingRuns();
  }

  // A drop that reaches the window navigates a WKWebView to file://… and
  // takes the whole app state with it: no router, no recovery. Drop
  // targets call stopPropagation(), so this only ever sees strays.
  const swallowDrag = (e: DragEvent) => e.preventDefault();

  // A hub-routed MUTATION timed out (`E_HUB_TIMEOUT` with
  // `details.outcome_unknown`): the hub may have done it anyway. Unlike
  // `onFocus`, this is not throttled by a clock — the whole point is that the
  // outcome is unknown right now, not on the next alt-tab.
  //
  // It is bounded in the only two ways that cannot lose a refresh: a window
  // whose configured hub is unusable fetches nothing at all (same rule as
  // `onFocus`), and a refresh this listener already started is not started a
  // second time while it is still in flight. The second matters because the
  // refresh is itself two hub-routed reads: a hub answering slowly would
  // otherwise get one full fleet re-fetch per timed-out call, each able to
  // time out in turn.
  let outcomeRefreshInFlight = false;
  function onOutcomeUnknown() {
    if (get(hubStatus).unavailable) return;
    if (outcomeRefreshInFlight) return;
    outcomeRefreshInFlight = true;
    bumpWorkChanged('resync');
    void Promise.all([loadProjects(), loadSessions()]).finally(() => {
      outcomeRefreshInFlight = false;
    });
  }

  onMount(() => {
    window.addEventListener('focus', onFocus);
    window.addEventListener('keydown', onKeydown);
    // Capture phase: the app chords must beat the terminal's own keydown
    // handler (same approach as the quick switcher).
    window.addEventListener('keydown', onChordKeydown, true);
    window.addEventListener('dragover', swallowDrag);
    window.addEventListener('drop', swallowDrag);
    window.addEventListener('fleet:outcome-unknown', onOutcomeUnknown);
  });

  // Opening a session from anywhere (sidebar, quick switcher, a Hosts-view
  // session row, a fresh create) means "go to it": leave the Hosts view so
  // the terminal shows that session.
  const unsubOpened = onSessionOpened(() => {
    closeHosts();
    leave('board');
    leave('accounts');
    leave('control');
    // Automation is a fleet page like Control: an opened session replaces
    // it (review r07).
    leave('automation');
  });
  // "View sessions" (host_actions.ts, called from anywhere: the `s` key,
  // HostDetail's header button) can't reach `closeHosts` directly — it asks
  // through this signal instead, same shape as `onSessionOpened` above.
  // Expanding the sidebar belongs HERE, not at one call site: the action has
  // just narrowed the sidebar to a host, and a collapsed rail would hide the
  // very list it filtered.
  const unsubHostsClose = onHostsCloseRequested(() => {
    sidebarCollapsed = false;
    closeHosts();
  });

  onDestroy(() => {
    window.removeEventListener('focus', onFocus);
    window.removeEventListener('keydown', onKeydown);
    window.removeEventListener('keydown', onChordKeydown, true);
    window.removeEventListener('dragover', swallowDrag);
    window.removeEventListener('drop', swallowDrag);
    window.removeEventListener('fleet:outcome-unknown', onOutcomeUnknown);
    unsubOpened();
    unsubHostsClose();
    unlistenEvents?.();
    unlistenVoice?.();
    setGapHandler(null);
  });

  function onResizeSidebar(delta: number) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + delta));
  }
  function onResizeCenter(delta: number) {
    centerPx = Math.max(220, Math.min(800, centerPx + delta));
  }

  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
  }
  function toggleCenter() {
    centerCollapsed = !centerCollapsed;
  }

  // Which view owns the right column: the Session tab or one overlay over it
  // (redesign step 3.1, `lib/destination.ts`). The store outlives a mount,
  // so a fresh App starts on the Session tab as the old per-mount flags did.
  destination.set('session');
  const filesMode = $derived($destination === 'files');
  const hostsMode = $derived($destination === 'hosts');
  const assetsMode = $derived($destination === 'assets');
  const boardMode = $derived($destination === 'board');
  const newLayout = $derived($uiLayout === 'new');
  // A row's ⋯ menu asks Details to run an action (step 3.10): Details must be
  // showing to take it.
  // In the New layout that is the inspector, or the Details tab when the
  // inspector has no room.
  const unsubRowAction = sessionActionRequest.subscribe((r) => {
    if (!r) return;
    centerCollapsed = false;
    if ($uiLayout === 'new') {
      if (inspectorRoom) inspectorOpen = true;
      else if (!detailsMain) goTo('details');
    }
  });
  onDestroy(unsubRowAction);
  // The Accounts page (step 4.1) is New-layout only until the rail (3.2);
  // switching back to Classic leaves it.
  const accountsMode = $derived($destination === 'accounts');
  // The Details tab (step 3.5): the New layout's session details in the
  // right column, in place of the inspector beside it.
  const detailsMode = $derived($destination === 'details');
  // Control (step 9.1) is the New layout's: Classic keeps the agent's sheet.
  const controlMode = $derived($destination === 'control');
  // Automation (step 8.4): a fleet page like Control, reached from the rail.
  const automationMode = $derived($destination === 'automation');
  $effect(() => {
    if ($uiLayout !== 'new')
      untrack(() => {
        leave('accounts');
        leave('details');
        leave('control');
      });
  });
  // Classic has no Inbox (step 3.3): it shows the Sessions list instead.
  $effect(() => {
    if ($uiLayout !== 'new' && $sidebarView === 'inbox') untrack(() => sidebarView.set('sessions'));
  });

  // Files mode swaps the center + terminal region for the worktree file
  // viewer. The Files tab needs a selected session (the worktree to browse);
  // deselecting one drops back to the terminal automatically.
  // Primitive projections of the selection: `$selectedSession` changes
  // identity on every `session:updated`, but these only change (and re-run
  // the effects below) when the fact they carry does.
  const selId = $derived($selectedSession?.id ?? null);
  const selNoPane = $derived(!!$selectedSession && hasNoPane($selectedSession));
  const selHasClaudeId = $derived(!!$selectedSession?.claude_session_id);
  // Multi-user M1: what this client may do with the selected row — `own`,
  // `drive`, `watch`, or `null` when it cannot tell (an unreachable hub, or
  // one that has not said who this device is). DERIVED, never a field on the
  // row: see `lib/access.ts` for why the row could not carry it. Read through
  // `$accessOf` so a `grant:changed` re-evaluates it with no re-list.
  const selAccess = $derived($accessOf($selectedSession));
  // Sharing never confers a terminal (spec §4.3 invariant 4): `pty_open` is
  // this machine's own `ssh … tmux attach`, which the hub cannot revoke. So a
  // row the client does not own gets the read-only snapshot instead, and
  // TerminalView is never mounted for it at all.
  const selOwned = $derived(selAccess === 'own');
  const selWatchOnly = $derived(!!$selectedSession && !selNoPane && !selOwned);
  // The picker's choices live on the hub when paired: re-read them when the
  // connection comes (back).
  let lastHubState: string | null = null;
  $effect(() => {
    const st = $hubConnection.state;
    if (lastHubState !== null && st !== lastHubState) void loadProjectPicks();
    lastHubState = st;
  });
  $effect(() => {
    if (selId === null || selNoPane) untrack(() => leave('files'));
  });

  // Hosts mode reuses the Files-mode mechanism: the center pane collapses and
  // an opaque overlay covers the terminal, which stays mounted so its PTY
  // survives the round trip. Hosts is fleet-scoped, so unlike Files it never
  // needs a selected session. Files and Hosts are mutually exclusive.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const hostsChord = hostsChordLabel(isMac);
  const sessionViewChord = sessionViewChordLabel(isMac);
  let hostsPreselect = $state<string | null>(null);
  // Assets mode shows the asset catalog. Like Hosts it is fleet-scoped (no
  // selected session needed) and renders as an opaque overlay over the
  // terminal, which stays mounted so its PTY survives the round trip.

  // Conversation and Terminal are two views of one session under a single
  // Session tab, so neither is "no mode set": which one shows is the stored
  // preference, narrowed by what this row can actually offer. Conversation
  // reuses the Files overlay for a tmux row, so the PTY stays mounted
  // underneath. Unlike Files/Hosts the Session tab keeps the center
  // (Details) pane — both its views are views *of* the session. The board
  // covers the Session tab without leaving it, so its segment stays shown.
  const sessionTabActive = $derived(
    !filesMode && !assetsMode && !hostsMode && !accountsMode && !detailsMode && !controlMode && !automationMode,
  );
  const effectiveView = $derived(
    resolveSessionView($sessionView, selNoPane, selHasClaudeId, selOwned),
  );
  const conversationMode = $derived(sessionTabActive && effectiveView === 'conversation');
  // Bumped to remount the view when a request names a host while it is open.
  let hostsViewKey = $state(0);
  /** Last host shown in the Hosts view, for this app session only. */
  let lastViewedHost: string | null = null;
  /** What had focus when Hosts opened (normally the terminal). */
  let hostsReturnFocus: HTMLElement | null = null;
  // Leaving Hosts any way other than closeHosts() drops the focus to restore.
  $effect(() => {
    if (!hostsMode) hostsReturnFocus = null;
  });

  function openHosts(host: string | null = null) {
    const preselect = host ?? $selectedSession?.host_alias ?? lastViewedHost ?? null;
    if (hostsMode) {
      // Already open: only a request naming a host changes anything.
      if (host !== null) {
        hostsPreselect = host;
        hostsViewKey++;
      }
      return;
    }
    const active = document.activeElement;
    hostsReturnFocus = active instanceof HTMLElement && active !== document.body ? active : null;
    hostsPreselect = preselect;
    goTo('hosts');
  }

  function closeHosts(restoreFocus = true) {
    if (!hostsMode) return;
    const el = hostsReturnFocus;
    leave('hosts');
    hostsReturnFocus = null;
    if (restoreFocus && el) {
      void tick().then(() => {
        if (el.isConnected) el.focus();
      });
    }
  }

  function toggleHosts() {
    if (hostsMode) closeHosts();
    else openHosts();
  }

  // Requests from outside App (quick switcher, Settings, onboarding card).
  $effect(() => {
    const req = $hostsViewRequest;
    if (!req) return;
    hostsViewRequest.set(null);
    untrack(() => openHosts(req.host));
  });

  // The quick switcher's asset and command rows (Assets M6, R19): open the
  // Assets overlay. The request is left set: AssetsPanel takes it, also when
  // it mounts only because of this.
  $effect(() => {
    if ($assetsViewRequest) untrack(showAssets);
  });

  // A path clicked in the Conversation tab: show Files for that session
  // (FilesPanel picks the path up and clears the request).
  $effect(() => {
    const req = $openPathRequest;
    if (!req) return;
    untrack(() => {
      // A pane-less row (bg / external) has no Files tab to hand this to.
      if ($selectedSession?.id === req.sessionId && !selNoPane) showFiles();
      else openPathRequest.set(null);
    });
  });

  function showSession() {
    // Hosts first: closing it is what restores the focus it took.
    closeHosts();
    goTo('session');
  }
  function showFiles() {
    if (!$selectedSession) return;
    goTo('files');
  }
  // Every Assets entry point (the Classic tab, the sidebar, the quick
  // switcher) opens Toolkit's Assets tab in the New layout (step 3.16).
  function showAssets() {
    toolkitTab.set('assets');
    goTo('assets');
  }
  // The rail's Toolkit reopens the tab it showed last.
  function showToolkit() {
    closeHosts();
    goTo('assets');
  }
  function showAccounts() {
    // Hosts first, as for the Session tab: closing it restores its focus.
    closeHosts();
    goTo('accounts');
  }
  // The New layout's rail (step 3.2). Inbox (3.3), Sessions and Work pick
  // the sidebar's list, as ⌘⇧W does, and bring back the Session tab from a
  // fleet page (Accounts, Hosts, Assets); over a session (Files, the board)
  // they leave the right column alone.
  function onRailSelect(id: RailId) {
    if (id === 'inbox' || id === 'sessions' || id === 'work') {
      sidebarCollapsed = false;
      sidebarView.set(id);
      if (hostsMode || accountsMode || assetsMode || controlMode || automationMode) showSession();
    } else if (id === 'control') {
      closeHosts();
      goTo('control');
    } else if (id === 'automation') {
      closeHosts();
      goTo('automation');
    } else if (id === 'accounts') showAccounts();
    else if (id === 'toolkit') showToolkit();
    else if (id === 'settings') settingsOpen.set(true);
  }
  // The task board (sprints design 2026-09-28 §6c) is an overlay over the
  // terminal like Assets, opened from the Work view's Board button
  // (`workBoardOpen`, a view of the same destination store); it keeps the
  // center pane, where a card opens its task. Opening it leaves the other
  // overlays, and each of them closes it.
  /**
   * Pick a sub-view. A row that cannot show it is left alone. The pref is
   * written only when the row can genuinely offer both views: on a row that
   * forces one of them, that view is already showing and already checked, so
   * a click is a no-op that must not silently overwrite the preference a
   * different row is relying on. showSession() still runs unconditionally —
   * the click should always leave whatever overlay was open.
   *
   * A row this client may only WATCH (multi-user M1) forces neither view even
   * with no `claude_session_id`: the terminal slot holds the read-only pane
   * snapshot, which needs no transcript, so `resolveSessionView` returns the
   * preference for both. By this comment's own rule the pref must therefore be
   * writable — otherwise the slot a watcher came for is only reachable when
   * the preference already happened to be `terminal`, and the toggle is inert.
   */
  function setSessionView(v: SessionView) {
    if (resolveSessionView(v, selNoPane, selHasClaudeId, selOwned) !== v) return;
    if (!selNoPane && (selHasClaudeId || selWatchOnly)) sessionView.set(v);
    showSession();
  }
  /**
   * ⌘J. Leaving an overlay (Files/Assets/Hosts) returns you to the view you
   * left, not somewhere else — it should feel like closing a window, not
   * navigating. The flip is reserved for the second press, once the Session
   * tab is already showing.
   */
  function flipSessionView() {
    if (!sessionTabActive || boardMode) {
      showSession();
      return;
    }
    setSessionView(otherSessionView(effectiveView));
  }
  const NO_PANE_TITLE = 'Runs outside tmux — no terminal';

  // ── The New layout's session tabs and inspector (step 3.5) ──
  // The inspector is Classic's Details pane moved beside the session, 280 to
  // 320 px, on ⌥⌘B / Ctrl+Alt+B. What fills a whole column (Today, a task,
  // the empty state, the Details tab) shows in the right column instead, so
  // it is never squeezed into the inspector, and Details mounts once.
  let inspectorOpen = $state(readPref('layout.inspector', true, isBool));
  $effect(() => {
    writePref('layout.inspector', inspectorOpen);
  });
  const taskShowing = $derived($sidebarView === 'work' && !!$selectedTaskId && $taskDetailOpen && !$todayOpen);
  const wideMode = $derived(filesMode || hostsMode || assetsMode || accountsMode || controlMode || automationMode);
  const detailsMain = $derived(
    newLayout && !wideMode && !boardMode && (detailsMode || $todayOpen || taskShowing || !$selectedSession),
  );
  // Review r08: a board card's task opens in the inspector column beside the
  // board (Classic shows it in the centre pane), whatever the inspector pref.
  const boardTask = $derived(newLayout && boardMode && taskShowing);
  const inspectorRoom = $derived(
    boardTask || (newLayout && !!$selectedSession && !wideMode && !boardMode && !detailsMain),
  );
  const inspectorShown = $derived(boardTask || (inspectorRoom && inspectorOpen));
  // Step 5.3: the pane's shells, for the Terminals tab. It is current while
  // the pane shows one of them (alone or split beside the agent).
  const selTerminals = $derived(
    $terminalPane.sessionId != null && $terminalPane.sessionId === $selectedSession?.id ? $terminalPane : null,
  );
  const terminalsShown = $derived(selTerminals?.active != null);
  const currentTab: SessionTab | null = $derived(
    detailsMode
      ? 'details'
      : filesMode
        ? 'files'
        : sessionTabActive && !boardMode
          ? effectiveView === 'conversation'
            ? 'conversation'
            : terminalsShown
              ? 'terminals'
              : 'agent'
          : null,
  );
  const tabDisabled = $derived<Partial<Record<SessionTab, string>>>({
    ...(!selHasClaudeId && !selNoPane ? { conversation: 'No Claude session id yet' } : {}),
    ...(selNoPane ? { agent: NO_PANE_TITLE, files: NO_PANE_TITLE, terminals: NO_PANE_TITLE } : {}),
    ...(!selNoPane && !selTerminals ? { terminals: 'Terminals open only on a session that is yours' } : {}),
  });
  const selName = $derived(
    $selectedSession
      ? ($showFriendlyNames && $selectedSession.friendly_name) || $selectedSession.tmux_name
      : '',
  );
  function onSessionTab(tab: SessionTab) {
    if (tab === 'conversation') setSessionView('conversation');
    else if (tab === 'agent') {
      setSessionView('terminal');
      requestTerminalTab('agent');
    } else if (tab === 'terminals') {
      setSessionView('terminal');
      requestTerminalTab('shells');
    } else if (tab === 'files') showFiles();
    else {
      closeHosts();
      goTo('details');
    }
  }
  function toggleInspector() {
    if (newLayout) inspectorOpen = !inspectorOpen;
    else toggleCenter();
  }
  /** The Terminal pill's tooltip on a session shared with this person: it is
   *  still a view of the pane, just not a live one (multi-user M1). */
  const WATCH_ONLY_TITLE = 'Shared with you — a read-only snapshot of the pane, not a terminal';

  // Footer usage segment: whether to look at usage, not the numbers. A coarse
  // clock is enough for "3m" ages and staleness.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  // What the agent is told about where the person is standing. `branch` has
  // no plumbing in App.svelte today (no per-session/current-branch state to
  // read), so it is left null here rather than adding new state for it.
  const agentContextInput: AgentContextInput = $derived({
    view: hostsMode ? 'hosts' : 'terminal',
    session: $selectedSession,
    hostAlias: $selectedSession?.host_alias ?? hostsPreselect,
    branch: null,
    // The app's one name policy: the chip and the prompt prefix name the
    // session the same way the sidebar row and the terminal header do.
    friendly: $showFriendlyNames,
  });

  function onHostsFilterSidebar(alias: string) {
    // `viewHostSessions` fires `onHostsCloseRequested`, which expands the
    // sidebar and closes the overlay — the `s` key and HostDetail's button
    // are the same path.
    viewHostSessions(alias);
  }
  function onHostsNewSession(alias: string) {
    requestNewSessionOnHost(alias);
  }

  // The Tidy-up sheet lives in the sidebar: a request for it (the Today
  // view's Stale section) brings a collapsed sidebar back so it can open.
  $effect(() => {
    if ($tidyRequest) sidebarCollapsed = false;
  });

  // The Add project dialog is mounted by the Sidebar, which is unmounted while
  // the rail is collapsed: a request from the switcher's Add row brings it
  // back, and the mounted Sidebar then consumes the request.
  $effect(() => {
    if ($addProjectRequest) sidebarCollapsed = false;
  });

  // Review r08: Settings (and Get started) are mounted by the Sidebar too, and
  // the rail's Settings and ⌘, must open them while it is collapsed.
  $effect(() => {
    if ($settingsOpen) sidebarCollapsed = false;
  });

  // "Insert into composer" (work graph M9.2) shows where the text went: the
  // selected session's conversation, over Today if it was open.
  let lastInsertSeq = 0;
  $effect(() => {
    const ins = $composerInsert;
    if (!ins || ins.seq === lastInsertSeq) return;
    lastInsertSeq = ins.seq;
    if ($selectedSession?.id !== ins.sessionId) return;
    todayOpen.set(false);
    setSessionView('conversation');
  });

  function onChordKeydown(e: KeyboardEvent) {
    const chord = appChord(e, isMac);
    if (!chord) return;
    // Another modal owns the keyboard while open; don't open a view (or a
    // second dialog) underneath it.
    if ((e.target as Element | null)?.closest?.('dialog')) return;
    e.preventDefault();
    e.stopPropagation();
    if (chord === 'hosts') toggleHosts();
    else if (chord === 'session-view') flipSessionView();
    else if (chord === 'settings') settingsOpen.set(true);
    // The New layout's ⌘E opens Control (step 9.1); Classic keeps the sheet.
    else if (chord === 'agent') {
      if (newLayout) toggleControl('chat');
      else void toggleAgent();
    }
    // The Work view has its own org filter: the chord cycles that one there.
    else if (chord === 'scope') (get(sidebarView) === 'work' ? cycleWorkOrg : cycleScope)();
    else if (chord === 'today') toggleToday();
    else if (chord === 'inspector') toggleInspector();
    else if (chord === 'open-in-editor') void openInEditorIfAllowed($selectedSession, selAccess);
    else if (chord === 'work-view') {
      sidebarCollapsed = false;
      toggleSidebarView();
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    const target = e.target as HTMLElement | null;
    // The agent panel is a fixed sheet over every view, so it takes Esc
    // before Files / Assets / Hosts do. AgentPanel handles the key itself
    // when focus is inside it (including in its composer, where the rule
    // below would otherwise leave the person stuck); this branch is for an
    // Esc with focus left on the page behind it. A modal <dialog> above the
    // sheet still owns its own Esc, and an editable outside the panel keeps
    // Esc for itself, exactly as Files and Assets do.
    if (!newLayout && $agentPanelOpen && !e.defaultPrevented) {
      if (target?.closest?.('dialog')) return;
      if (!isEditable(target)) {
        closeAgent();
        return;
      }
    }
    // Esc leaves files mode (the terminal is covered while it's open, so Esc
    // can't be meant for the terminal here) — but not while the user is
    // typing in a field such as the file filter, where Esc belongs to that
    // input and exiting the whole panel would be surprising.
    if (filesMode) {
      if (isEditable(target)) return;
      leave('files');
      return;
    }
    // The board, like Assets: Esc closes it, not while typing or in a
    // dialog (a drag in progress takes its own Esc first).
    if (boardMode && !newLayout && !e.defaultPrevented) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      leave('board');
      return;
    }
    // Assets is an overlay with no Esc handling of its own; the same rule as
    // Files applies (not while typing in the catalog's filter field).
    if (assetsMode) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      leave('assets');
      return;
    }
    // Accounts, the same rule as Assets.
    if (accountsMode) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      leave('accounts');
      return;
    }
    // Control, the same rule (its composer keeps Esc while you type).
    if (controlMode) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      leave('control');
      return;
    }
    if (automationMode) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      leave('automation');
      return;
    }
    // Inside the Hosts view, HostsView owns Esc (back to the list, clear the
    // filter, close from the list). This catches only an Esc with focus lost
    // to the page or left on the right column's chrome; a dialog, an input
    // or the sidebar keep their own Esc.
    if (hostsMode && !e.defaultPrevented) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      if (target?.closest?.('[data-testid="hosts-view"]')) return;
      const onPage = !target || target === document.body || target === document.documentElement;
      if (onPage || target.closest('[data-testid="pane-terminal"]')) closeHosts();
    }
  }

  // Build the grid template based on which panes are collapsed. We keep a
  // constant 5-column layout (panel, resizer, panel, resizer, panel; the New
  // layout puts its rail in front as a sixth) so the grid placement of each
  // named child stays stable across toggles. Setting a slot to `0px`
  // effectively hides it while preserving column count.
  const gridTemplate = $derived.by(() => {
    const sb = sidebarCollapsed ? '20px' : `${sidebarPx}px`;
    const sbResizer = sidebarCollapsed ? '0px' : '4px';
    // In files mode the center pane collapses to zero — the file viewer
    // takes the whole region right of the sidebar.
    const wide = filesMode || hostsMode || assetsMode || accountsMode || controlMode || automationMode;
    const center = wide ? '0px' : centerCollapsed ? '20px' : `${centerPx}px`;
    const centerResizer = wide || centerCollapsed ? '0px' : '4px';
    // The New layout (steps 3.2, 3.5): the rail in front, no center pane,
    // and the inspector after the session.
    if ($uiLayout === 'new') {
      const insp = inspectorShown ? 'minmax(var(--inspector-min), var(--inspector-max))' : '0px';
      return `var(--rail-w) ${sb} ${sbResizer} 1fr 0px ${insp}`;
    }
    return `${sb} ${sbResizer} ${center} ${centerResizer} 1fr`;
  });
</script>

<HintLayer />
<Toasts />
<TransferSheet />
<!-- One Share sheet for the whole app, opened by id from `shareSheetFor`
     (multi-user M1) — the same shape as the Transfer sheet above, and for the
     same reason: the row, the details panel and anything else that wants to
     share a session should not each own a dialog. -->
<ShareSheet />
<McpConfirmDialog />
<!-- In the New layout the agent lives in Control (step 9.1), on the rail. -->
{#if !newLayout}
  <AgentFab />
  <AgentPanel contextInput={agentContextInput} />
{/if}
<!-- Cmd/Ctrl+K / Cmd/Ctrl+P, and the one place a project is picked for a new
     session (the sidebar's "+ New session" and the Hosts view's `n` open it
     in New session mode). Its rows publish a request that mounts the dialog
     here. Since redesign 1.9 this is the only mount: a project row's `+` and
     Add project publish the same request. -->
<QuickSwitcher />
<ShortcutSheet />
{#if $newSessionRequest}
  <NewSessionDialog
    project={$newSessionRequest.project}
    initialName={$newSessionRequest.initialName}
    initialHost={$newSessionRequest.initialHost}
    ticket={$newSessionRequest.ticket}
    autostart={$newSessionRequest.autostart}
    proposal={$newSessionRequest.proposal}
    onCreate={(s) => {
      clearNewSessionRequest();
      selectSessionExplicitly(s);
    }}
    onCancel={clearNewSessionRequest}
  />
{/if}

{#if showWelcome}
  <!-- "Skip for now" closes the welcome dialog but intentionally leaves the
       sidebar "Get started" card visible (it sets welcomed, not dismissed). -->
  <WelcomeDialog
    onstart={() => {
      onboardingWelcomed.set(true);
      onboardingDismissed.set(false);
      showWelcome = false;
    }}
    onskip={() => {
      onboardingWelcomed.set(true);
      showWelcome = false;
    }}
  />
{/if}

{#if $uiLayout === 'new'}
  <!-- Redesign 3.17: the Main board's header, above everything else. -->
  <ShellHeader mac={isMac} />
  <StartupSplash onhubsettings={() => settingsOpen.set(true)} />
  {#if revealVersion}
    <UpdateReveal version={revealVersion} onclose={() => (revealVersion = null)} />
  {/if}
{/if}
{#if $hubStatus.remote}
  <HubConnectionBanner hubUrl={$hubStatus.url} />
{/if}
{#if $hubStatus.unavailable}
  <HubUnavailableBanner
    reason={$hubStatus.unavailable}
    hubUrl={$hubStatus.configured_url}
    onsettings={() => settingsOpen.set(true)} />
{/if}
<main class="layout" class:with-header={$uiLayout === 'new'} style="grid-template-columns: {gridTemplate};">
  {#if $uiLayout === 'new'}
    <AppRail {isMac} onselect={onRailSelect} />
  {/if}
  {#if sidebarCollapsed}
    <button
      class="strip-expand"
      onclick={toggleSidebar}
      title="Show sidebar"
      aria-label="Show sidebar"
      data-testid="sidebar-expand"
    >›</button>
    <!-- 0-width resizer slot, keeps grid stable. -->
    <div></div>
  {:else}
    <Pane id="sidebar" fullBleed>
      {#snippet children()}
        <Sidebar onCollapse={toggleSidebar} />
      {/snippet}
    </Pane>
    <Resizer id="sidebar" onresize={onResizeSidebar} />
  {/if}

  <!-- The New layout has no center pane (step 3.5): Details is the
       inspector after the session, or fills the right column. -->
  {#if newLayout}
    <!-- nothing -->
  {:else if filesMode || hostsMode || accountsMode}
    <!-- Center collapsed to 0 in files/hosts/accounts mode — two empty grid cells. -->
    <div></div>
    <div></div>
  {:else if centerCollapsed}
    <button
      class="strip-expand left-edge"
      onclick={toggleCenter}
      title="Show details"
      aria-label="Show details pane"
      data-testid="center-expand"
    >›</button>
    <div></div>
  {:else}
    <Pane id="center">
      {#snippet children()}
        <div class="center-wrap">
          <button
            class="center-collapse"
            onclick={toggleCenter}
            title="Hide details (more room for terminal)"
            aria-label="Hide details pane"
            data-testid="center-collapse"
          >‹</button>
          <Details />
        </div>
      {/snippet}
    </Pane>
    <Resizer id="center" onresize={onResizeCenter} />
  {/if}

  <div class="right-col" data-testid="pane-terminal">
    {#if newLayout}
      <SessionTabs
        session={$selectedSession}
        name={selName}
        current={currentTab}
        terminalCount={selTerminals?.shells.length ?? 0}
        disabled={tabDisabled}
        assetsActive={assetsMode}
        {inspectorOpen}
        inspectorAvailable={inspectorRoom}
        {isMac}
        onselect={onSessionTab}
        onassets={showAssets}
        oninspector={toggleInspector}
      />
    {:else}
    <div class="view-tabs" role="tablist">
      <button
        class="view-tab"
        class:active={sessionTabActive && !boardMode}
        role="tab"
        aria-selected={sessionTabActive && !boardMode}
        title={!$selectedSession ? 'No session selected' : 'The running session — its conversation and its terminal'}
        onclick={showSession}
        data-testid="tab-session">Session</button
      >
      <button
        class="view-tab"
        class:active={filesMode}
        role="tab"
        aria-selected={filesMode}
        disabled={!$selectedSession || selNoPane}
        title={!$selectedSession
          ? 'Select a session first'
          : selNoPane
            ? NO_PANE_TITLE
            : 'Browse the session worktree'}
        onclick={showFiles}
        data-testid="tab-files">Files</button
      >
      <!-- Fleet-scoped like Hosts: never disabled, no selected session needed. -->
      <button
        class="view-tab"
        class:active={assetsMode && !hostsMode}
        role="tab"
        aria-selected={assetsMode && !hostsMode}
        title="The asset catalog and its per-host drift state"
        onclick={showAssets}
        data-testid="tab-assets">Assets</button
      >
      <!-- Always present so Hosts keeps its place; the segment inside it
           appears only while the Session tab owns the panel. Not a nested
           tablist — two tablists in one strip would have a screen reader
           announce two independent tab positions for one place. -->
      <div class="tab-tail">
        {#if sessionTabActive && $selectedSession}
          <div class="subtabs" role="radiogroup" aria-label="Session view">
            <button
              class="subtab"
              class:active={effectiveView === 'conversation'}
              role="radio"
              aria-checked={effectiveView === 'conversation'}
              aria-keyshortcuts={isMac ? 'Meta+J' : 'Control+Shift+J'}
              disabled={!selHasClaudeId && !selNoPane}
              title={!selHasClaudeId
                ? selNoPane
                  ? 'No transcript yet — nothing to show'
                  : 'No Claude session id yet'
                : `Claude conversation from the transcript (${sessionViewChord})`}
              onclick={() => setSessionView('conversation')}
              data-testid="subtab-conversation">Conversation</button
            >
            <button
              class="subtab"
              class:active={effectiveView === 'terminal'}
              role="radio"
              aria-checked={effectiveView === 'terminal'}
              aria-keyshortcuts={isMac ? 'Meta+J' : 'Control+Shift+J'}
              disabled={selNoPane}
              title={selNoPane
                ? NO_PANE_TITLE
                : selWatchOnly
                  ? `${WATCH_ONLY_TITLE} (${sessionViewChord})`
                  : `The tmux pane (${sessionViewChord})`}
              onclick={() => setSessionView('terminal')}
              data-testid="subtab-terminal">Terminal</button
            >
          </div>
        {/if}
      </div>
      <!-- Fleet-scoped, so set apart on the right and never disabled. -->
      <button
        class="view-tab hosts-tab"
        class:active={hostsMode}
        role="tab"
        aria-selected={hostsMode}
        aria-keyshortcuts={isMac ? 'Meta+I' : 'Control+Shift+H'}
        title="Every host, grouped by Claude account ({hostsChord})"
        onclick={toggleHosts}
        data-testid="tab-hosts">Hosts <kbd>{hostsChord}</kbd></button
      >
    </div>
    {/if}
    <div class="right-body">
      {#if $selectedSession && selNoPane}
        <!-- Rows with no pane (bg agents, external Claude sessions) have no
             PTY. We intentionally do NOT mount TerminalView here so pty_open
             is never attempted (it would error with "no tmux"). The tradeoff:
             selecting such a row unmounts the terminal, so returning to a
             normal session reconnects its PTY. The Conversation is the only
             view these rows have. -->
        <div class="view-slot">
          <ConversationPanel session={$selectedSession} visible={!hostsMode && !assetsMode && !accountsMode && !controlMode && !automationMode} />
        </div>
      {:else}
        <!-- TerminalView stays mounted underneath so the PTY and its ANSI
             buffer survive a Files-mode round trip — flipping back is instant
             and never re-fits or reconnects the terminal. -->
        <div class="view-slot">
          {#if selWatchOnly && $selectedSession}
            <!-- The third view state (multi-user M1): a session reached
                 through a GRANT. TerminalView is not mounted — not hidden,
                 not disabled, not mounted — because mounting it is what
                 attaches: `openTerm` fires off the selection and calls
                 `pty_open` with no gesture, and that attach is this machine's
                 own SSH, which the hub can neither refuse nor revoke. The
                 read-only snapshot takes its place.

                 Expressed as a nested branch rather than a third top-level
                 one so the Files and Conversation overlays below stay shared:
                 a duplicated copy of them would be a second place to keep in
                 step. They are NOT ungated, though — the earlier claim that
                 "both are routed reads the hub authorises per call" was wrong
                 about one path. FilesPanel's writes are `local_only`, so a
                 paired desktop is refused them by the hub. ConversationPanel
                 reads are routed, but its composer is not only routed: the
                 outbox uploads the attachment tray with `upload_attachments`
                 (`same_in_both` — this machine's own scp onto the owner's
                 host, no hub in the path) BEFORE the routed `send_prompt`. So
                 the composer gates itself on this client's access to the row,
                 inside ConversationPanel, which is where its controls are.
                 The guarantee the plan asks for here — "the component is
                 never mounted for a granted session" — is about TerminalView
                 and is the same either way. -->
            <WatchView
              session={$selectedSession}
              access={selAccess}
              visible={!hostsMode && !assetsMode && !accountsMode && !controlMode && !automationMode && !filesMode && !conversationMode}
            />
          {:else}
            <TerminalView />
          {/if}
        </div>
        {#if filesMode && $selectedSession}
          <div class="view-slot overlay">
            <FilesPanel session={$selectedSession} />
          </div>
        {/if}
        {#if conversationMode && $selectedSession}
          <div class="view-slot overlay">
            <ConversationPanel session={$selectedSession} visible={!hostsMode && !assetsMode && !accountsMode && !controlMode && !automationMode} onOpenTerminal={() => setSessionView('terminal')} />
          </div>
        {/if}
      {/if}
      {#if hostsMode}
        <div class="view-slot overlay" data-testid="hosts-overlay">
          {#key hostsViewKey}
            <HostsView
              preselect={hostsPreselect}
              onClose={() => closeHosts()}
              onFilterSidebar={onHostsFilterSidebar}
              onNewSession={onHostsNewSession}
              onSelectionChange={(alias) => (lastViewedHost = alias)}
            />
          {/key}
        </div>
      {/if}
      {#if assetsMode}
        <div class="view-slot overlay" data-testid="assets-overlay">
          {#if newLayout}
            <Toolkit visible={assetsMode} />
          {:else}
            <AssetsPanel visible={assetsMode} />
          {/if}
        </div>
      {/if}
      {#if accountsMode}
        <div class="view-slot overlay" data-testid="accounts-overlay">
          <AccountsPage />
        </div>
      {/if}
      {#if controlMode}
        <div class="view-slot overlay" data-testid="control-overlay">
          <ControlView {isMac} contextInput={agentContextInput} />
        </div>
      {/if}
      {#if automationMode}
        <div class="view-slot overlay" data-testid="automation-overlay">
          <AutomationView />
        </div>
      {/if}
      {#if boardMode}
        <!-- In the New layout the board is a Work view (step 3.10): its Work
             tab opens it, the other tabs leave it, and it has no close of
             its own. It still sits over the mounted terminal. -->
        <div class="view-slot overlay" data-testid={newLayout ? 'board-view' : 'board-overlay'}>
          <WorkBoard onclose={newLayout ? undefined : () => leave('board')} />
        </div>
      {/if}
      {#if detailsMain}
        <!-- The New layout's Details tab, and what fills a column (Today, a
             task, the empty state): over the mounted terminal like the rest. -->
        <div class="view-slot overlay" data-testid="details-view">
          <Details />
        </div>
      {/if}
    </div>
  </div>
  {#if newLayout}
    <!-- The center resizer's 0-width slot, then the inspector (step 3.5). -->
    <div></div>
    {#if inspectorShown}
      <aside class="inspector" data-testid="inspector" aria-label="Inspector">
        <Details />
      </aside>
    {:else}
      <div></div>
    {/if}
  {/if}
</main>

{#if $downloadsOpen}
  <DownloadsSheet onclose={() => downloadsOpen.set(false)} />
{/if}

<footer class="status">
  <StatusBarMark />
  <!-- Review r13 (step 1.3): a sentence and the next step; the codes stay
       under Details. -->
  {#if healthError}
    <span class="err" data-testid="health-error"
      >Couldn't check the backend: {healthText}. <details class="status-details"
        ><summary>Details</summary>{healthError}</details
      ></span
    >
  {:else if bootstrapError}
    <span class="err" data-testid="bootstrap-error"
      >Couldn't load {bootstrapWhat ?? 'the fleet'}.
      <button type="button" class="status-retry" data-testid="bootstrap-retry" disabled={bootstrapRetrying} onclick={() => void retryBootstrap()}
        >{bootstrapRetrying ? 'Trying…' : 'Retry'}</button
      >
      <details class="status-details"><summary>Details</summary>{bootstrapError}</details></span
    >
  {:else if health}
    <!-- In remote mode these are the HUB's version, database and schema, not
         this app's — `health_check` routes to the hub's `fleet_health`. The
         line says so itself now (`app 0.4.5 · hub 0.4.6 · …`): the badge
         beside it names WHICH hub, which was never the same as saying whose
         version the reader is looking at. -->
    <span data-testid="footer-version" title={versions.title}>{versions.text}</span>
    <button
      type="button"
      class="hub-badge"
      data-testid="footer-downloads"
      title="Files sessions sent to your devices"
      onclick={() => downloadsOpen.set(true)}
      >⤓ Downloads…{unseenDownloads > 0 ? ` (${unseenDownloads})` : ''}</button
    >
    {#if trackersLine}
      <!-- Work graph M12.4: the tracker roll-up, re-read by TrackerAttention. -->
      <button
        type="button"
        class="hub-badge"
        class:err={($trackersHealth?.failing ?? 0) > 0}
        data-testid="footer-trackers"
        title="Tracker sync health. Settings → Trackers to reconnect."
        onclick={() => openSettingsAt('settings.trackers')}>{trackersLine}</button
      >
    {/if}
  {:else if $hubStatus.unavailable}
    <!-- No health to report, so no hub version and no schema — but which app
         this is stays worth saying, and it is the first thing anyone asks
         when the hub it was paired to will not come up. -->
    {#if $appVersion}
      <span data-testid="footer-version" title={versions.title}>{versions.text}</span>
    {/if}
    <!-- Not "connecting…": nothing is, and nothing will until Settings. -->
    <button
      type="button"
      class="hub-badge err"
      data-testid="footer-hub-unavailable"
      title={$hubStatus.unavailable}
      onclick={() => settingsOpen.set(true)}>hub unavailable — managing no fleet</button
    >
  {:else}
    <span class="muted">connecting…</span>
  {/if}
  {#if $hubStatus.remote}
    <!-- The spec's header badge. It lives in the status bar because that is
         the one strip always on screen, and because the plaintext warning
         has to sit beside the hub it is about. -->
    <button
      type="button"
      class="hub-badge"
      data-testid="hub-badge"
      title="This window is a client of {$hubStatus.url}. Settings → Hub to disconnect."
      onclick={() => settingsOpen.set(true)}
      >hub: {$hubStatus.url}{$hubStatus.client_name ? ` (as ${$hubStatus.client_name})` : ''}</button
    >
    {#if $hubStatus.warning}
      <!-- Every launch, not once in a log file: the operator opted in to
           sending a fleet-wide credential in the clear, and a decision
           nobody is ever reminded of stops being a decision. -->
      <span class="err hub-warning" data-testid="hub-warning" title={$hubStatus.warning}
        >⚠ {$hubStatus.warning}</span
      >
    {/if}
  {/if}
  <!-- The usage segment is the embed page `embed.status_footer`. -->
  <EmbedSlot
    slot="status_footer"
    ctx={{
      now: nowSec,
      hosts: $hosts,
      accounts: $accounts,
      snapshots: $accountUsage,
      onopenhost: (host) => openHosts(host),
    }}
  />
  {#if $uiLayout === 'new'}
    <!-- The manual's StatusBar ends on the shortcuts sheet (3.17). -->
    <button
      type="button"
      class="hub-badge footer-end"
      data-testid="footer-shortcuts"
      title="Keyboard shortcuts  ?"
      onclick={() => shortcutSheetOpen.set(true)}>? Shortcuts…</button
    >
  {/if}
</footer>

<style>
  .layout {
    display: grid;
    /* The footer is fixed at the bottom; this is the rest. Read off the same
       token the footer sizes itself from — hardcoding 24px here left the page
       1px taller than the viewport, because the footer's border was not in it. */
    height: calc(100vh - var(--status-h));
    width: 100vw;
    background: var(--bg);
  }
  /* The new layout's header (3.17) occupies --header-h above the grid. */
  .layout.with-header {
    height: calc(100vh - var(--status-h) - var(--header-h));
  }
  .status {
    /* border-box: --status-h is the occupied height, border included. */
    box-sizing: border-box;
    height: var(--status-h);
    line-height: calc(var(--status-h) - 1px);
    padding: 0 0.75rem;
    background: var(--bg-pane);
    border-top: 1px solid var(--border);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    display: flex;
    align-items: center;
    gap: 1rem;
  }
  .footer-end {
    margin-left: auto;
  }
  .status .err { color: var(--danger); }
  .status-details { display: inline; }
  .status-details summary { display: inline; cursor: pointer; }
  .status-retry {
    font: inherit;
    padding: 0 var(--space-1);
    border: 1px solid currentColor;
    border-radius: var(--radius-xs);
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .hub-badge {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0 0.4rem;
    font: inherit;
    color: var(--fg-muted);
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 28rem;
  }
  .hub-badge:hover { color: var(--fg); }
  .hub-warning {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40vw;
  }

  /* Collapsed-pane strip: a thin always-visible vertical button. Same
     visual language for both sidebar and center collapse so the user
     learns one interaction. */
  .strip-expand {
    background: var(--bg-pane);
    border: none;
    border-right: 1px solid var(--border);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-md);
    line-height: 1;
    padding: 0;
    writing-mode: vertical-rl;
    text-orientation: mixed;
  }
  .strip-expand:hover {
    color: var(--fg);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-pane));
  }
  /* When the center pane is collapsed, its strip sits between the sidebar
     and the terminal — flip the border to its LEFT edge so the strip looks
     attached to the terminal side. */
  .strip-expand.left-edge {
    border-right: none;
    border-left: 1px solid var(--border);
  }

  .center-wrap {
    position: relative;
    height: 100%;
    overflow: auto;
    padding-right: 1rem;
  }
  .center-collapse {
    position: absolute;
    top: 0.4rem;
    right: 0.2rem;
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-sm);
    line-height: 1;
    z-index: 2;
  }
  .center-collapse:hover { color: var(--fg); border-color: var(--accent); }

  /* Right column: a thin Terminal/Files tab strip above the body. */
  .inspector {
    min-width: 0;
    min-height: 0;
    overflow: auto;
    border-left: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .right-col {
    display: flex;
    flex-direction: column;
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
  .view-tabs {
    display: flex;
    flex: 0 0 auto;
    gap: 1px;
    padding: 0.2rem 0.35rem 0;
    background: var(--bg-pane);
    border-bottom: 1px solid var(--border);
  }
  .view-tab {
    background: transparent;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.25rem 0.8rem;
  }
  .view-tab:hover:not(:disabled) { color: var(--fg); }
  .view-tab.active {
    background: var(--bg);
    border-color: var(--border);
    color: var(--fg);
    /* Sit on top of the strip's bottom border. */
    margin-bottom: -1px;
    padding-bottom: calc(0.25rem + 1px);
  }
  .view-tab:disabled { opacity: 0.4; cursor: not-allowed; }
  /* Claims the free space so Hosts stays pinned right whether or not the
     segment is showing. */
  .tab-tail {
    margin-left: auto;
    display: flex;
    align-items: center;
  }
  /* A pill, deliberately unlike the tabs above it: this is a switch within
     the active tab, not a sibling of it. */
  .subtabs {
    display: flex;
    gap: 1px;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 1px;
    margin-bottom: 0.2rem;
  }
  .subtab {
    background: transparent;
    border: none;
    border-radius: var(--radius-pill);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0.1rem 0.6rem;
  }
  .subtab:hover:not(:disabled) { color: var(--fg); }
  .subtab.active {
    background: var(--bg);
    color: var(--fg);
  }
  .subtab:disabled { opacity: 0.4; cursor: not-allowed; }
  .hosts-tab {
    margin-left: 0.75rem;
    position: relative;
  }
  /* A thin rule sets the fleet-scoped tab apart from the session tabs. */
  .hosts-tab::before {
    content: '';
    position: absolute;
    left: -0.5rem;
    top: 0.3rem;
    bottom: 0.3rem;
    border-left: 1px solid var(--border);
  }
  .hosts-tab kbd {
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    margin-left: 0.25rem;
  }

  .right-body {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }
  /* Both slots fill the body; the Files overlay (opaque) covers the
     terminal while active rather than unmounting/resizing it. */
  .view-slot {
    position: absolute;
    inset: 0;
  }
  .view-slot.overlay {
    z-index: 2;
    background: var(--bg);
  }
</style>
