<script lang="ts">
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';
  import { theme, cycleTheme } from './lib/theme';

  let sidebarPx = $state(280);
  let centerPx = $state(360);

  function onResizeSidebar(delta: number) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + delta));
  }
  function onResizeCenter(delta: number) {
    centerPx = Math.max(220, Math.min(800, centerPx + delta));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="claude-fleet" empty="No projects yet">
    <button class="theme-toggle" onclick={cycleTheme} title="Theme: {$theme}">
      theme: {$theme}
    </button>
  </Pane>
  <Resizer id="sidebar" onresize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" onresize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
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
