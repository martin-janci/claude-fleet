<script lang="ts">
  import { applyWorkEvents, loadTrackers, sessionsMentioning } from './lib/trackers';
  import { loadOrgs, cycleScope } from './lib/orgs';
  import type { WorkEvent } from './lib/trackers';
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import { todayOpen } from './lib/today';
  import { composerInsert } from './lib/conversation';
  import TerminalView from './lib/TerminalView.svelte';
  import FilesPanel from './lib/FilesPanel.svelte';
  import HostsView from './lib/HostsView.svelte';
  import ConversationPanel from './lib/ConversationPanel.svelte';
  import AssetsPanel from './lib/AssetsPanel.svelte';
  import { loadProjects, applyProjectEvents } from './lib/projects';
  import { loadSessions, applySessionEvents, sessions, hasNoPane, showFriendlyNames, sidebarGroupBy } from './lib/sessions';
  import { loadHosts, applyHostEvents, hosts } from './lib/hosts';
  import { viewHostSessions } from './lib/host_actions';
  import { loadAccounts, applyAccountEvents, accounts } from './lib/accounts';
  import { loadTasks, applyTaskEvents } from './lib/tasks';
  import { loadAccountUsage, applyAccountUsageEvents, accountUsage } from './lib/account_usage_store';
  import { footerUsage } from './lib/usage_glance';
  import { mergeInventoryRow, clearInventoryFor, loadAssets, syncProgress, repoStatus } from './lib/assets';
  import { subscribeToRowEvents } from './lib/events';
  import TransferSheet from './lib/TransferSheet.svelte';
  import { applyMoveProgress, recheckWaitingRuns } from './lib/moves';
  import { dispatchTimelineEvents, dispatchConversationsChanged } from './lib/live_events';
  import Toasts from './lib/Toasts.svelte';
  import QuickSwitcher from './lib/QuickSwitcher.svelte';
  import NewSessionDialog from './lib/NewSessionDialog.svelte';
  import { newSessionRequest, clearNewSessionRequest } from './lib/new_session_request';
  import { push, pushError } from './lib/toasts';
  import type { Result } from './lib/result';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { selectedSession, restoreLastSession, selectSessionExplicitly, onSessionOpened } from './lib/selection';
  import {
    appChord,
    hostsChordLabel,
    hostsViewOpen,
    hostsViewRequest,
    onHostsCloseRequested,
    openPathRequest,
    requestNewSessionOnHost,
    sessionViewChordLabel,
    settingsOpen,
  } from './lib/app_views';
  import { detectMac, isEditable } from './lib/terminal_keys';
  import { loadSessionUi, saveSessionUi, DEFAULT_UI } from './lib/session_ui';
  import { readPref, writePref, sessionView } from './lib/prefs';
  import { resolveSessionView, otherSessionView, type SessionView } from './lib/session_view';
  import WelcomeDialog from './lib/WelcomeDialog.svelte';
  import HintLayer from './lib/HintLayer.svelte';
  import McpConfirmDialog from './lib/McpConfirmDialog.svelte';
  import AgentFab from './lib/AgentFab.svelte';
  import AgentPanel from './lib/AgentPanel.svelte';
  import { agentPanelOpen, closeAgent, toggleAgent } from './lib/operator';
  import type { AgentContextInput } from './lib/agent_context';
  import { onboardingWelcomed, onboardingDismissed } from './lib/onboarding';
  import { hubStatus, loadHubStatus } from './lib/hub';
  import HubUnavailableBanner from './lib/HubUnavailableBanner.svelte';
  import { startHubConnection } from './lib/hub_connection';
  import HubConnectionBanner from './lib/HubConnectionBanner.svelte';
  import { get } from 'svelte/store';

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
  let healthError = $state<string | null>(null);
  // Bootstrap (initial list_* fetches) failures. These used to be swallowed,
  // so a broken DB showed an innocent "No projects yet". Now they surface as
  // a sticky error toast (with the E_* code) plus this footer banner.
  let bootstrapError = $state<string | null>(null);
  let unlistenEvents: UnlistenFn | null = null;
  let showWelcome = $state(false);

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
  });

  // `work:*` frames: trackers and their first sync. When a tracker finishes
  // its FIRST sync, the sessions whose keys it owns just got titles and
  // status (retro-binding); say how many, and offer the Group-by-Work view.
  function onWorkEvents(events: WorkEvent[]) {
    for (const t of applyWorkEvents(events)) {
      const keys = get(sessions).map((s) => s.work?.key ?? null);
      const { count, prefixes } = sessionsMentioning(t, keys);
      if (count === 0) continue;
      push({
        kind: 'info',
        message: `${count} session${count === 1 ? '' : 's'} mention ${prefixes.map((p) => `${p}-*`).join(', ')}`,
        action: { label: 'Review', run: () => sidebarGroupBy.set('work') },
      });
    }
  }

  onMount(async () => {
    // FIRST, and awaited: the rest of this function branches on it. A hub
    // client must not poll account usage (the backend refuses it, so it would
    // be an error toast on every launch for a panel that does not apply), and
    // the footer names the hub it is a window onto.
    await loadHubStatus();
    // A hub is configured but this launch could not use it. The backend owns
    // nothing and refuses every fleet command, so each load below would only
    // add an error toast under the banner that already explains all of them.
    // The window shows that banner and the way to Settings, and nothing else.
    if (get(hubStatus).unavailable) return;
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
    } else if (hr0.error.code === 'E_HUB_CONTRACT') {
      healthFailure = `health: ${hr0.error.code}`;
    } else {
      healthError = `${hr0.error.code}: ${hr0.error.message}`;
      push({ kind: 'error', code: hr0.error.code, message: `Health check failed: ${hr0.error.message}` });
    }
    // Subscribed BEFORE the first list: a `session:updated` that lands while
    // the list is in flight would otherwise be emitted to no listener and
    // lost until the row changes again.
    unlistenEvents = await subscribeToRowEvents({
      onSessionEvents: applySessionEvents,
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
    });
    const [pr, sr, hr, ar] = await Promise.all([
      loadProjects(),
      loadSessions(),
      loadHosts(),
      loadAccounts(),
    ]);
    const failures = [
      healthFailure,
      reportBootstrap('projects', pr),
      reportBootstrap('sessions', sr),
      reportBootstrap('hosts', hr),
      reportBootstrap('accounts', ar),
    ].filter((f): f is string => f !== null);
    if (failures.length > 0) bootstrapError = `startup load failed — ${failures.join(', ')}`;
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
  const unsubOpened = onSessionOpened(() => closeHosts());
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

  // Files mode swaps the center + terminal region for the worktree file
  // viewer. The Files tab needs a selected session (the worktree to browse);
  // deselecting one drops back to the terminal automatically.
  let filesMode = $state(false);
  // Primitive projections of the selection: `$selectedSession` changes
  // identity on every `session:updated`, but these only change (and re-run
  // the effects below) when the fact they carry does.
  const selId = $derived($selectedSession?.id ?? null);
  const selNoPane = $derived(!!$selectedSession && hasNoPane($selectedSession));
  const selHasClaudeId = $derived(!!$selectedSession?.claude_session_id);
  $effect(() => {
    if (selId === null || selNoPane) filesMode = false;
  });

  // Hosts mode reuses the Files-mode mechanism: the center pane collapses and
  // an opaque overlay covers the terminal, which stays mounted so its PTY
  // survives the round trip. Hosts is fleet-scoped, so unlike Files it never
  // needs a selected session. Files and Hosts are mutually exclusive.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const hostsChord = hostsChordLabel(isMac);
  const sessionViewChord = sessionViewChordLabel(isMac);
  let hostsMode = $state(false);
  let hostsPreselect = $state<string | null>(null);
  // Assets mode shows the asset catalog. Like Hosts it is fleet-scoped (no
  // selected session needed) and renders as an opaque overlay over the
  // terminal, which stays mounted so its PTY survives the round trip.
  let assetsMode = $state(false);

  // Conversation and Terminal are two views of one session under a single
  // Session tab, so neither is "no mode set": which one shows is the stored
  // preference, narrowed by what this row can actually offer. Conversation
  // reuses the Files overlay for a tmux row, so the PTY stays mounted
  // underneath. Unlike Files/Hosts the Session tab keeps the center
  // (Details) pane — both its views are views *of* the session.
  const sessionTabActive = $derived(!filesMode && !assetsMode && !hostsMode);
  const effectiveView = $derived(resolveSessionView($sessionView, selNoPane, selHasClaudeId));
  const conversationMode = $derived(sessionTabActive && effectiveView === 'conversation');
  // Bumped to remount the view when a request names a host while it is open.
  let hostsViewKey = $state(0);
  /** Last host shown in the Hosts view, for this app session only. */
  let lastViewedHost: string | null = null;
  /** What had focus when Hosts opened (normally the terminal). */
  let hostsReturnFocus: HTMLElement | null = null;
  $effect(() => {
    hostsViewOpen.set(hostsMode);
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
    filesMode = false;
    assetsMode = false;
    hostsMode = true;
  }

  function closeHosts(restoreFocus = true) {
    if (!hostsMode) return;
    hostsMode = false;
    const el = hostsReturnFocus;
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
    filesMode = false;
    assetsMode = false;
    closeHosts();
  }
  function showFiles() {
    if (!$selectedSession) return;
    closeHosts(false);
    assetsMode = false;
    filesMode = true;
  }
  function showAssets() {
    closeHosts(false);
    filesMode = false;
    assetsMode = true;
  }
  /**
   * Pick a sub-view. A row that cannot show it is left alone. The pref is
   * written only when the row can genuinely offer both views: on a row that
   * forces one of them, that view is already showing and already checked, so
   * a click is a no-op that must not silently overwrite the preference a
   * different row is relying on. showSession() still runs unconditionally —
   * the click should always leave whatever overlay was open.
   */
  function setSessionView(v: SessionView) {
    if (resolveSessionView(v, selNoPane, selHasClaudeId) !== v) return;
    if (!selNoPane && selHasClaudeId) sessionView.set(v);
    showSession();
  }
  /**
   * ⌘J. Leaving an overlay (Files/Assets/Hosts) returns you to the view you
   * left, not somewhere else — it should feel like closing a window, not
   * navigating. The flip is reserved for the second press, once the Session
   * tab is already showing.
   */
  function flipSessionView() {
    if (!sessionTabActive) {
      showSession();
      return;
    }
    setSessionView(otherSessionView(effectiveView));
  }
  const NO_PANE_TITLE = 'Runs outside tmux — no terminal';

  // Footer usage segment: whether to look at usage, not the numbers. A coarse
  // clock is enough for "3m" ages and staleness.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const usageFooter = $derived(footerUsage($hosts, $accounts, $accountUsage, nowSec));

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
    sidebarCollapsed = false;
    requestNewSessionOnHost(alias);
  }

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
    else if (chord === 'agent') void toggleAgent();
    else if (chord === 'scope') cycleScope();
    else if (chord === 'today') todayOpen.update((v) => !v);
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
    if ($agentPanelOpen && !e.defaultPrevented) {
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
      filesMode = false;
      return;
    }
    // Assets is an overlay with no Esc handling of its own; the same rule as
    // Files applies (not while typing in the catalog's filter field).
    if (assetsMode) {
      if (isEditable(target) || target?.closest?.('dialog')) return;
      assetsMode = false;
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
  // constant 5-column layout (panel, resizer, panel, resizer, panel) so the
  // grid placement of each named child stays stable across toggles. Setting
  // a slot to `0px` effectively hides it while preserving column count.
  const gridTemplate = $derived.by(() => {
    const sb = sidebarCollapsed ? '20px' : `${sidebarPx}px`;
    const sbResizer = sidebarCollapsed ? '0px' : '4px';
    // In files mode the center pane collapses to zero — the file viewer
    // takes the whole region right of the sidebar.
    const wide = filesMode || hostsMode || assetsMode;
    const center = wide ? '0px' : centerCollapsed ? '20px' : `${centerPx}px`;
    const centerResizer = wide || centerCollapsed ? '0px' : '4px';
    return `${sb} ${sbResizer} ${center} ${centerResizer} 1fr`;
  });
</script>

<HintLayer />
<Toasts />
<TransferSheet />
<McpConfirmDialog />
<AgentFab />
<AgentPanel contextInput={agentContextInput} />
<!-- Cmd/Ctrl+K / Cmd/Ctrl+P. Its "new session" rows publish a request that
     mounts the dialog here (the Sidebar keeps its own instance for its
     footer button until it adopts the store post-#46). -->
<QuickSwitcher />
{#if $newSessionRequest}
  <NewSessionDialog
    project={$newSessionRequest.project}
    initialName={$newSessionRequest.initialName}
    initialHost={$newSessionRequest.initialHost}
    ticket={$newSessionRequest.ticket}
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

{#if $hubStatus.remote}
  <HubConnectionBanner hubUrl={$hubStatus.url} />
{/if}
{#if $hubStatus.unavailable}
  <HubUnavailableBanner
    reason={$hubStatus.unavailable}
    hubUrl={$hubStatus.configured_url}
    onsettings={() => settingsOpen.set(true)} />
{/if}
<main class="layout" style="grid-template-columns: {gridTemplate};">
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

  {#if filesMode || hostsMode}
    <!-- Center collapsed to 0 in files/hosts mode — two empty grid cells. -->
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
    <div class="view-tabs" role="tablist">
      <button
        class="view-tab"
        class:active={sessionTabActive}
        role="tab"
        aria-selected={sessionTabActive}
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
              title={selNoPane ? NO_PANE_TITLE : `The tmux pane (${sessionViewChord})`}
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
    <div class="right-body">
      {#if $selectedSession && selNoPane}
        <!-- Rows with no pane (bg agents, external Claude sessions) have no
             PTY. We intentionally do NOT mount TerminalView here so pty_open
             is never attempted (it would error with "no tmux"). The tradeoff:
             selecting such a row unmounts the terminal, so returning to a
             normal session reconnects its PTY. The Conversation is the only
             view these rows have. -->
        <div class="view-slot">
          <ConversationPanel session={$selectedSession} visible={!hostsMode && !assetsMode} />
        </div>
      {:else}
        <!-- TerminalView stays mounted underneath so the PTY and its ANSI
             buffer survive a Files-mode round trip — flipping back is instant
             and never re-fits or reconnects the terminal. -->
        <div class="view-slot">
          <TerminalView />
        </div>
        {#if filesMode && $selectedSession}
          <div class="view-slot overlay">
            <FilesPanel session={$selectedSession} />
          </div>
        {/if}
        {#if conversationMode && $selectedSession}
          <div class="view-slot overlay">
            <ConversationPanel session={$selectedSession} visible={!hostsMode && !assetsMode} onOpenTerminal={() => setSessionView('terminal')} />
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
          <AssetsPanel visible={assetsMode} />
        </div>
      {/if}
    </div>
  </div>
</main>

<footer class="status">
  {#if healthError}
    <span class="err" data-testid="health-error">ipc error: {healthError}</span>
  {:else if bootstrapError}
    <span class="err" data-testid="bootstrap-error">{bootstrapError}</span>
  {:else if health}
    <!-- In remote mode this is the HUB's version, database and schema, not
         this app's — `health_check` routes to the hub's `fleet_health`. The
         badge beside it is what says whose. -->
    <span>v{health.version} · db: {health.db_ready ? 'ok' : 'fail'} · schema {health.schema_version}</span>
  {:else if $hubStatus.unavailable}
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
  {#if usageFooter}
    <button
      type="button"
      class="usage-seg tone-{usageFooter.tone}"
      data-testid="footer-usage"
      data-state={usageFooter.state}
      aria-label={usageFooter.ariaLabel}
      title={usageFooter.ariaLabel}
      onclick={() => openHosts(usageFooter.host)}>{usageFooter.text}</button
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
  .status {
    /* border-box: --status-h is the occupied height, border included. */
    box-sizing: border-box;
    height: var(--status-h);
    line-height: calc(var(--status-h) - 1px);
    padding: 0 0.75rem;
    background: var(--bg-pane);
    border-top: 1px solid var(--border);
    font-size: 0.75rem;
    color: var(--fg-muted);
    display: flex;
    align-items: center;
    gap: 1rem;
  }
  .status .err { color: #e64a4a; }
  .hub-badge {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
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
  .usage-seg {
    margin-left: auto;
    background: transparent;
    border: none;
    padding: 0 0.3rem;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: var(--fg);
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .usage-seg:hover { text-decoration: underline; }
  .usage-seg.tone-muted { color: var(--fg-muted); }
  .usage-seg.tone-warn { color: var(--usage-warn); }
  .usage-seg.tone-alarm { color: var(--usage-crit); }

  /* Collapsed-pane strip: a thin always-visible vertical button. Same
     visual language for both sidebar and center collapse so the user
     learns one interaction. */
  .strip-expand {
    background: var(--bg-pane);
    border: none;
    border-right: 1px solid var(--border);
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 1rem;
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
    border-radius: 4px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.9rem;
    line-height: 1;
    z-index: 2;
  }
  .center-collapse:hover { color: var(--fg); border-color: var(--accent); }

  /* Right column: a thin Terminal/Files tab strip above the body. */
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
    border-radius: 5px 5px 0 0;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.74rem;
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
    border-radius: 999px;
    padding: 1px;
    margin-bottom: 0.2rem;
  }
  .subtab {
    background: transparent;
    border: none;
    border-radius: 999px;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.7rem;
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
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.65rem;
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
