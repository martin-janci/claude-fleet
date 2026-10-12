<!-- Moving a session (redesign step 5.13, LoadersInUse board): the kit's
     Host stream loader (`<Loader name="host-stream">`) between the host it
     leaves and the host it goes to, while the move runs. The loader brings
     the kit's delay, reduced motion and pausing; `paused` stops it where it
     is. The step list beside it says how far the move got: this only says
     that it is moving. -->
<script lang="ts">
  import Loader from './Loader.svelte';

  let { from, to, paused = false }: { from: string; to: string; paused?: boolean } = $props();
</script>

<div class="hs" data-testid="host-stream">
  <span class="host">{from}</span>
  <Loader name="host-stream" {paused} delay={0} label="Moving from {from} to {to}" testid="host-stream-loader" />
  <span class="host">{to}</span>
</div>

<style>
  .hs {
    display: flex;
    align-items: center;
    gap: var(--space-2, 8px);
    font-size: var(--text-2xs);
    color: var(--fg-2);
  }
  .host {
    flex: none;
    font-weight: 600;
  }
</style>
