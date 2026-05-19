<script lang="ts">
  import Pane from './lib/Pane.svelte';
  import Resizer from './lib/Resizer.svelte';

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
  <Pane id="sidebar" title="Projects" empty="No projects yet" />
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
</style>
