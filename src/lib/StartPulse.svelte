<!-- A ⌘N start in flight (redesign step 5.13): the Pulse sequence driven by
     the backend's `start:progress` frames, and — while the worktree step is
     running — the kit's Hex field, the manual's loader for building a
     checkout (motion.md: "Building or checking: worktree setup"). Both move
     only when a frame arrives. -->
<script lang="ts">
  import Loader from './Loader.svelte';
  import PulseSteps from './PulseSteps.svelte';
  import { startPulse } from './session_loaders';
  import type { CreatingStart } from './sessions';

  let { start, title, testid = 'new-session-pulse' }: { start: CreatingStart; title: string; testid?: string } = $props();

  const pulse = $derived(startPulse(start.steps, start.kind));
  const buildingWorktree = $derived(start.steps.worktree === 'started');
</script>

<div class="sp">
  <PulseSteps {pulse} {title} {testid} />
  {#if buildingWorktree}
    <Loader name="hex-field" size={64} label="Setting up worktree" testid="start-hex" />
  {/if}
</div>

<style>
  .sp {
    display: flex;
    align-items: center;
    gap: var(--space-3, 12px);
  }
</style>
