<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import TerminalView from './lib/TerminalView.svelte';
  import { loadProjects, bootstrapProjects, mergeProject, mergeWorktree } from './lib/projects';
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
  $effect(() => {
    writePref('layout.sidebar', sidebarPx);
  });
  $effect(() => {
    writePref('layout.sidebar-collapsed', sidebarCollapsed);
  });

  // Center pane state is PER SESSION — the user said "Kazda session ma mat
  // aj vlastnu pamat UI, nastavenia rozdelenia". When the user picks a
  // session we hydrate from localStorage; when they resize or toggle
  // collapse we persist back under that session's key.
  let centerPx = $state(DEFAULT_UI.centerPx);
  let centerCollapsed = $state(DEFAULT_UI.centerCollapsed);

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
    centerCollapsed = ui.centerCollapsed;
    queueMicrotask(() => {
      hydrating = false;
    });
  });

  $effect(() => {
    const sess = $selectedSession;
    if (!sess || hydrating) return;
    saveSessionUi(sess.host_alias, sess.tmux_name, { centerPx, centerCollapsed });
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
      onProjectUpdated: mergeProject,
      onWorktreeUpdated: mergeWorktree,
    });
  });

  function onFocus() {
    void loadProjects();
    void loadSessions();
  }

  onMount(() => {
    window.addEventListener('focus', onFocus);
  });

  onDestroy(() => {
    window.removeEventListener('focus', onFocus);
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

  // Build the grid template based on which panes are collapsed. We keep a
  // constant 5-column layout (panel, resizer, panel, resizer, panel) so the
  // grid placement of each named child stays stable across toggles. Setting
  // a slot to `0px` effectively hides it while preserving column count.
  const gridTemplate = $derived.by(() => {
    const sb = sidebarCollapsed ? '20px' : `${sidebarPx}px`;
    const sbResizer = sidebarCollapsed ? '0px' : '4px';
    const center = centerCollapsed ? '20px' : `${centerPx}px`;
    const centerResizer = centerCollapsed ? '0px' : '4px';
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

  {#if centerCollapsed}
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

  <Pane id="terminal" fullBleed>
    {#snippet children()}
      <TerminalView />
    {/snippet}
  </Pane>
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
</style>
