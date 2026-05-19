<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { theme, cycleTheme } from './lib/theme';
  import { healthCheck, type Health } from './lib/ipc';
  import Sidebar from './lib/Sidebar.svelte';
  import Details from './lib/Details.svelte';
  import TerminalView from './lib/TerminalView.svelte';
  import { loadProjects } from './lib/projects';
  import { loadSessions } from './lib/sessions';

  let sidebarPx = $state(280);
  let centerPx = $state(360);
  let health = $state<Health | null>(null);
  let healthError = $state<string | null>(null);

  onMount(async () => {
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = String(e);
    }
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
  });

  function onResizeSidebar(delta: number) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + delta));
  }
  function onResizeCenter(delta: number) {
    centerPx = Math.max(220, Math.min(800, centerPx + delta));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="claude-fleet">
    {#snippet children()}
      <Sidebar />
      <button class="theme-toggle" onclick={cycleTheme} title="Theme: {$theme}">
        theme: {$theme}
      </button>
    {/snippet}
  </Pane>
  <Resizer id="sidebar" onresize={onResizeSidebar} />
  <Pane id="center">
    {#snippet children()}
      <Details />
    {/snippet}
  </Pane>
  <Resizer id="center" onresize={onResizeCenter} />
  <Pane id="terminal">
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
  .theme-toggle {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }
</style>
