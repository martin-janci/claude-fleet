<script lang="ts">
  import { selectedProject, selectedSession } from './selection';
  import ProjectDetails from './ProjectDetails.svelte';
  import SessionDetails from './SessionDetails.svelte';
</script>

{#if $selectedSession}
  <SessionDetails session={$selectedSession} />
{:else if $selectedProject}
  <ProjectDetails />
{:else}
  <p class="empty" data-testid="details-empty">Pick a project or session to see details.</p>
{/if}

<style>
  .empty { color: var(--fg-muted); font-size: 0.9rem; }
</style>
