<script lang="ts">
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';

  let sidebarPx = 280;
  let centerPx = 360;

  function onResizeSidebar(e: CustomEvent<number>) {
    sidebarPx = Math.max(180, Math.min(640, sidebarPx + e.detail));
  }
  function onResizeCenter(e: CustomEvent<number>) {
    centerPx = Math.max(220, Math.min(800, centerPx + e.detail));
  }
</script>

<main class="layout" style="grid-template-columns: {sidebarPx}px 4px {centerPx}px 4px 1fr;">
  <Pane id="sidebar" title="Projects" empty="No projects yet" />
  <Resizer id="sidebar" on:resize={onResizeSidebar} />
  <Pane id="center" empty="Pick a session to see details" />
  <Resizer id="center" on:resize={onResizeCenter} />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
</style>
