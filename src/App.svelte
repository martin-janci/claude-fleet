<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import TerminalView from './lib/TerminalView.svelte';
  import FilesPanel from './lib/FilesPanel.svelte';
  import { loadProjects, bootstrapProjects, mergeProjectFromEvent, mergeWorktree } from './lib/projects';
  import { loadSessions, bootstrapSessions, mergeSession, removeSession } from './lib/sessions';
  import { bootstrapHosts, mergeHost, removeHost } from './lib/hosts';
  import { bootstrapAccounts, mergeAccount } from './lib/accounts';
  import { subscribeToRowEvents } from './lib/events';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { selectedSession } from './lib/selection';
  import { loadSessionUi, saveSessionUi, DEFAULT_UI } from './lib/session_ui';
  import { readPref, writePref } from './lib/prefs';

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

  // When the selected session changes, swap in its persisted layout. We use
  // a `hydrating` gate so the save-effect below doesn't immediately echo
  // the freshly-loaded values back to storage.
  let hydrating = false;
  $effect(() => {
    const sess = $selectedSession;
    if (!sess) return;
    hydrating = true;
    const ui = loadSessionUi(sess.host_alias, sess.tmux_name);
    centerPx = ui.centerPx;
    queueMicrotask(() => {
      hydrating = false;
    });
  });

  // centerPx changes per resize-drag frame too — debounce its persistence.
  let centerSaveTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const sess = $selectedSession;
    const px = centerPx;
    if (!sess || hydrating) return;
    clearTimeout(centerSaveTimer);
    centerSaveTimer = setTimeout(
      () => saveSessionUi(sess.host_alias, sess.tmux_name, { centerPx: px }),
      200,
    );
    return () => clearTimeout(centerSaveTimer);
  });

  let health = $state<Health | null>(null);
  let healthError = $state<string | null>(null);
  let unlistenEvents: UnlistenFn | null = null;

  onMount(async () => {
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = String(e);
    }
    await Promise.all([
      bootstrapProjects(),
      bootstrapSessions(),
      bootstrapHosts(),
      bootstrapAccounts(),
    ]);
    unlistenEvents = await subscribeToRowEvents({
      onSessionCreated: mergeSession,
      onSessionUpdated: mergeSession,
      onSessionKilled: (p) => removeSession(p.id),
      onHostAdded: mergeHost,
      onHostProbed: mergeHost,
      onHostRemoved: (p) => removeHost(p.alias),
      onAccountUpserted: mergeAccount,
      onProjectUpdated: mergeProjectFromEvent,
      onWorktreeUpdated: mergeWorktree,
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
    if (!$selectedSession) filesMode = false;
  });
  function showTerminal() {
    filesMode = false;
  }
  function showFiles() {
    if ($selectedSession) filesMode = true;
  }
  function onKeydown(e: KeyboardEvent) {
    // Esc leaves files mode (the terminal is covered while it's open, so Esc
    // can't be meant for the terminal here).
    if (e.key === 'Escape' && filesMode) {
      filesMode = false;
      e.stopPropagation();
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
        disabled={!$selectedSession}
        title={$selectedSession ? 'Browse the session worktree' : 'Select a session first'}
        onclick={showFiles}
        data-testid="tab-files">Files</button
      >
    </div>
    <div class="right-body">
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
    </div>
  </div>
</main>

<footer class="status">
  {#if healthError}
    <span class="err">ipc error: {healthError}</span>
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
