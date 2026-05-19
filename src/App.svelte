<script lang="ts">
  import Pane from './lib/Pane.svelte';
</script>

<main class="layout">
  <Pane id="sidebar" title="Projects" empty="No projects yet" />
  <Pane id="center" empty="Pick a session to see details" />
  <Pane id="terminal" empty="No terminal attached" />
</main>

<style>
  .layout {
    display: grid;
    grid-template-columns: var(--sidebar-w) var(--center-w) 1fr;
    height: 100vh;
    width: 100vw;
    background: var(--bg);
  }
</style>
