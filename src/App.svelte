<script lang="ts">
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import TerminalView from './lib/TerminalView.svelte';
  import FilesPanel from './lib/FilesPanel.svelte';
  import HostsView from './lib/HostsView.svelte';
  import ConversationPanel from './lib/ConversationPanel.svelte';
  import AssetsPanel from './lib/AssetsPanel.svelte';
  import { loadProjects, applyProjectEvents } from './lib/projects';
  import { loadSessions, applySessionEvents, sessions, hasNoPane } from './lib/sessions';
  import { loadHosts, applyHostEvents, hosts, hostFilter } from './lib/hosts';
  import { loadAccounts, applyAccountEvents, accounts } from './lib/accounts';
  import { loadTasks, applyTaskEvents } from './lib/tasks';
  import { loadAccountUsage, applyAccountUsageEvents, accountUsage } from './lib/account_usage_store';
  import { footerUsage } from './lib/usage_glance';
  import { mergeInventoryRow, clearInventoryFor, loadAssets, syncProgress, repoStatus } from './lib/assets';
  import { subscribeToRowEvents } from './lib/events';
  import Toasts from './lib/Toasts.svelte';
  import QuickSwitcher from './lib/QuickSwitcher.svelte';
  import NewSessionDialog from './lib/NewSessionDialog.svelte';
  import { newSessionRequest, clearNewSessionRequest } from './lib/new_session_request';
  import { push, pushError } from './lib/toasts';
  import type { Result } from './lib/result';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { selectedSession, restoreLastSession, selectSession, onSessionOpened } from './lib/selection';
  import {
    appChord,
    hostsChordLabel,
    hostsViewOpen,
    hostsViewRequest,
    openPathRequest,
    requestNewSessionOnHost,
    settingsOpen,
  } from './lib/app_views';
  import { detectMac, isEditable } from './lib/terminal_keys';
  import { loadSessionUi, saveSessionUi, DEFAULT_UI } from './lib/session_ui';
  import { readPref, writePref } from './lib/prefs';
  import WelcomeDialog from './lib/WelcomeDialog.svelte';
  import HintLayer from './lib/HintLayer.svelte';
  import McpConfirmDialog from './lib/McpConfirmDialog.svelte';
  import { onboardingWelcomed, onboardingDismissed } from './lib/onboarding';
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
    pushError(r.error, `Failed to load ${what}`);
    return `${what}: ${r.error.code}`;
  }

  onMount(async () => {
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = String(e);
      push({ kind: 'error', code: 'E_IPC', message: `Health check failed: ${String(e)}` });
    }
    const [pr, sr, hr, ar] = await Promise.all([
      loadProjects(),
      loadSessions(),
      loadHosts(),
      loadAccounts(),
    ]);
    const failures = [
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
    // Batched handlers: a reconcile burst of N `session:updated` events lands
    // as ONE store update instead of N (see events.ts).
    unlistenEvents = await subscribeToRowEvents({
      onSessionEvents: applySessionEvents,
      onHostEvents: applyHostEvents,
      onAccountEvents: applyAccountEvents,
      onProjectEvents: applyProjectEvents,
      onTaskEvents: applyTaskEvents,
      onAccountUsageEvents: applyAccountUsageEvents,
      onAssetInventoryUpdated: mergeInventoryRow,
      onAssetInventoryCleared: (p) => clearInventoryFor(p.host_alias, p.harness),
      onCatalogLoaded: () => { void loadAssets(); void repoStatus(); },
      onSyncProgress: (p) => syncProgress.set(p),
    });
    // Tasks are secondary to the session list: load after the row
    // subscription is live so no `task:updated` is missed, and never block
    // startup on it (a failure only leaves the Tasks panel empty).
    void loadTasks();
    // Account usage: same reasoning — not on the critical bootstrap path,
    // loaded after the subscription so no `account_usage:updated` is missed.
    void loadAccountUsage();
  });

  // Catch-up net for missed Tauri events (e.g. sleep/wake, dropped events).
  // With M3 events flowing, the store stays fresh by itself most of the time —
  // throttle the focus-driven re-fetch to 30s so alt-tabbing doesn't hammer
  // the backend with a full list_projects + list_sessions on every focus.
  let lastFocusFetch = 0;
  const FOCUS_FETCH_INTERVAL_MS = 30_000;
  function onFocus() {
    const now = Date.now();
    if (now - lastFocusFetch < FOCUS_FETCH_INTERVAL_MS) return;
    lastFocusFetch = now;
    void loadProjects();
    void loadSessions();
  }

  onMount(() => {
    window.addEventListener('focus', onFocus);
    window.addEventListener('keydown', onKeydown);
    // Capture phase: the app chords must beat the terminal's own keydown
    // handler (same approach as the quick switcher).
    window.addEventListener('keydown', onChordKeydown, true);
  });

  // Opening a session from anywhere (sidebar, quick switcher, a Hosts-view
  // session row, a fresh create) means "go to it": leave the Hosts view so
  // the terminal shows that session.
  const unsubOpened = onSessionOpened(() => closeHosts());

  onDestroy(() => {
    window.removeEventListener('focus', onFocus);
    window.removeEventListener('keydown', onKeydown);
    window.removeEventListener('keydown', onChordKeydown, true);
    unsubOpened();
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

  // Conversation mode shows the transcript-backed Conversation panel (spec
  // §6). For a tmux row it reuses the Files overlay, so the PTY stays mounted
  // underneath; Files, Hosts and Conversation are mutually exclusive. Unlike
  // Files/Hosts it keeps the center (Details) pane — it is a view of the
  // session, like the terminal. A row with no pane (bg / external) has no
  // terminal to show, so selecting one opens Conversation by default, and
  // moving from such a row to a tmux row drops back to the terminal.
  let conversationMode = $state(false);
  let prevNoPane = false;
  $effect(() => {
    void selId;
    const noPane = selNoPane;
    const hasId = selHasClaudeId;
    untrack(() => {
      if (noPane) {
        conversationMode = true;
        filesMode = false;
      } else if (prevNoPane) {
        conversationMode = false;
      }
      if (!noPane && !hasId) conversationMode = false;
    });
    prevNoPane = noPane;
  });

  // Hosts mode reuses the Files-mode mechanism: the center pane collapses and
  // an opaque overlay covers the terminal, which stays mounted so its PTY
  // survives the round trip. Hosts is fleet-scoped, so unlike Files it never
  // needs a selected session. Files and Hosts are mutually exclusive.
  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const hostsChord = hostsChordLabel(isMac);
  let hostsMode = $state(false);
  let hostsPreselect = $state<string | null>(null);
  // Assets mode shows the asset catalog. Like Hosts it is fleet-scoped (no
  // selected session needed) and renders as an opaque overlay over the
  // terminal, which stays mounted so its PTY survives the round trip.
  let assetsMode = $state(false);
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
    // A no-pane row has nothing but the Conversation under the Hosts overlay,
    // so it stays the view to return to; a tmux row returns to the terminal.
    if (!selNoPane) conversationMode = false;
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

  function showTerminal() {
    filesMode = false;
    conversationMode = false;
    assetsMode = false;
    closeHosts();
  }
  function showFiles() {
    if (!$selectedSession) return;
    closeHosts(false);
    conversationMode = false;
    assetsMode = false;
    filesMode = true;
  }
  function showConversation() {
    if (!$selectedSession?.claude_session_id) return;
    closeHosts(false);
    filesMode = false;
    assetsMode = false;
    conversationMode = true;
  }
  function showAssets() {
    closeHosts(false);
    filesMode = false;
    assetsMode = true;
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

  function onHostsFilterSidebar(alias: string) {
    sidebarCollapsed = false;
    hostFilter.set(alias);
  }
  function onHostsNewSession(alias: string) {
    sidebarCollapsed = false;
    requestNewSessionOnHost(alias);
  }

  function onChordKeydown(e: KeyboardEvent) {
    const chord = appChord(e, isMac);
    if (!chord) return;
    // Another modal owns the keyboard while open; don't open a view (or a
    // second dialog) underneath it.
    if ((e.target as Element | null)?.closest?.('dialog')) return;
    e.preventDefault();
    e.stopPropagation();
    if (chord === 'hosts') toggleHosts();
    else settingsOpen.set(true);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    const target = e.target as HTMLElement | null;
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
<McpConfirmDialog />
<!-- Cmd/Ctrl+K / Cmd/Ctrl+P. Its "new session" rows publish a request that
     mounts the dialog here (the Sidebar keeps its own instance for its
     footer button until it adopts the store post-#46). -->
<QuickSwitcher />
{#if $newSessionRequest}
  <NewSessionDialog
    project={$newSessionRequest.project}
    initialName={$newSessionRequest.initialName}
    onCreate={(s) => {
      clearNewSessionRequest();
      selectSession(s);
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
        class:active={!filesMode && !hostsMode && !conversationMode && !assetsMode}
        role="tab"
        aria-selected={!filesMode && !hostsMode && !conversationMode && !assetsMode}
        disabled={selNoPane}
        title={selNoPane ? NO_PANE_TITLE : undefined}
        onclick={showTerminal}
        data-testid="tab-terminal">Terminal</button
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
      <button
        class="view-tab"
        class:active={conversationMode && !hostsMode && !assetsMode}
        role="tab"
        aria-selected={conversationMode && !hostsMode && !assetsMode}
        disabled={!selHasClaudeId}
        title={!selHasClaudeId ? 'No Claude session id yet' : 'Claude conversation from the transcript'}
        onclick={showConversation}
        data-testid="tab-conversation">Conversation</button
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
            <ConversationPanel session={$selectedSession} visible={!hostsMode && !assetsMode} onOpenTerminal={showTerminal} />
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
    <span class="err">ipc error: {healthError}</span>
  {:else if bootstrapError}
    <span class="err" data-testid="bootstrap-error">{bootstrapError}</span>
  {:else if health}
    <span>v{health.version} · db: {health.db_ready ? 'ok' : 'fail'} · schema {health.schema_version}</span>
  {:else}
    <span class="muted">connecting…</span>
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
    height: calc(100vh - 24px);
    width: 100vw;
    background: var(--bg);
  }
  .status {
    height: 24px;
    line-height: 24px;
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
  .hosts-tab {
    margin-left: auto;
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
