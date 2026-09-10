<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import TerminalView from './lib/TerminalView.svelte';
  import BgSessionPanel from './lib/BgSessionPanel.svelte';
  import FilesPanel from './lib/FilesPanel.svelte';
  import { loadProjects, bootstrapProjects, applyProjectEvents } from './lib/projects';
  import { loadSessions, bootstrapSessions, applySessionEvents, sessions } from './lib/sessions';
  import { bootstrapHosts, applyHostEvents, hosts } from './lib/hosts';
  import { bootstrapAccounts, applyAccountEvents } from './lib/accounts';
  import { subscribeToRowEvents } from './lib/events';
  import Toasts from './lib/Toasts.svelte';
  import { push, pushError } from './lib/toasts';
  import type { Result } from './lib/result';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { selectedSession, restoreLastSession } from './lib/selection';
  import { loadSessionUi, saveSessionUi, DEFAULT_UI } from './lib/session_ui';
  import { readPref, writePref } from './lib/prefs';
  import WelcomeDialog from './lib/WelcomeDialog.svelte';
  import HintLayer from './lib/HintLayer.svelte';
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
      bootstrapProjects(),
      bootstrapSessions(),
      bootstrapHosts(),
      bootstrapAccounts(),
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
    const workSessionCount = get(sessions).filter((s) => s.kind !== 'bg').length;
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
    });
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
  });

  onDestroy(() => {
    window.removeEventListener('focus', onFocus);
    window.removeEventListener('keydown', onKeydown);
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
  $effect(() => {
    if (!$selectedSession || $selectedSession.kind === 'bg') filesMode = false;
  });
  function showTerminal() {
    filesMode = false;
  }
  function showFiles() {
    if ($selectedSession) filesMode = true;
  }
  function onKeydown(e: KeyboardEvent) {
    // Esc leaves files mode (the terminal is covered while it's open, so Esc
    // can't be meant for the terminal here) — but not while the user is
    // typing in a field such as the file filter, where Esc belongs to that
    // input and exiting the whole panel would be surprising.
    if (e.key === 'Escape' && filesMode) {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;
      filesMode = false;
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
    const center = filesMode ? '0px' : centerCollapsed ? '20px' : `${centerPx}px`;
    const centerResizer = filesMode || centerCollapsed ? '0px' : '4px';
    return `${sb} ${sbResizer} ${center} ${centerResizer} 1fr`;
  });
</script>

<HintLayer />
<Toasts />

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

  {#if filesMode}
    <!-- Center collapsed to 0 in files mode — two empty grid cells. -->
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
        class:active={!filesMode}
        role="tab"
        aria-selected={!filesMode}
        onclick={showTerminal}
        data-testid="tab-terminal">Terminal</button
      >
      <button
        class="view-tab"
        class:active={filesMode}
        role="tab"
        aria-selected={filesMode}
        disabled={!$selectedSession || $selectedSession.kind === 'bg'}
        title={!$selectedSession
          ? 'Select a session first'
          : $selectedSession.kind === 'bg'
            ? 'Not available for background sessions'
            : 'Browse the session worktree'}
        onclick={showFiles}
        data-testid="tab-files">Files</button
      >
    </div>
    <div class="right-body">
      {#if $selectedSession?.kind === 'bg'}
        <!-- Background sessions have no PTY. We intentionally do NOT mount
             TerminalView here so pty_open is never attempted (it would error
             with "no tmux"). The tradeoff: selecting a bg session unmounts the
             terminal, so returning to a normal session reconnects its PTY.
             Acceptable — bg agents run unattended and are rarely interleaved. -->
        <div class="view-slot">
          <BgSessionPanel session={$selectedSession} />
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
  }
  .status .err { color: #e64a4a; }

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
