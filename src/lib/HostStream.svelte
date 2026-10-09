<!-- Moving a session (redesign step 5.13, LoadersInUse board): particles
     stream from the host it leaves to the host it goes to while the move
     runs. Twelve dots, CSS only; reduced motion leaves them resting along
     the path, and `paused` stops them where they are. The step list beside
     it says how far the move got: this only says that it is moving. -->
<script lang="ts">
  import { effectiveMotion } from './motion';

  let { from, to, paused = false }: { from: string; to: string; paused?: boolean } = $props();

  const DOTS = 12;
  const still = $derived($effectiveMotion !== 'full');
</script>

<div
  class="hs"
  class:hs--still={still || paused}
  data-testid="host-stream"
>
  <span class="host">{from}</span>
  <span class="path" aria-hidden="true">
    {#each Array.from({ length: DOTS }, (_, i) => i) as i (i)}
      <i style:animation-delay="{(-i * 1.6) / DOTS}s" style:top="{(i * 37) % 9}px" style:--x="{(i * 100) / DOTS}%"></i>
    {/each}
  </span>
  <span class="host">{to}</span>
</div>

<style>
  .hs {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--text-2xs);
    color: var(--fg-2);
  }
  .host {
    flex: none;
    font-weight: 600;
  }
  .path {
    position: relative;
    flex: 1 1 auto;
    min-width: 80px;
    height: 12px;
    overflow: hidden;
  }
  .path i {
    position: absolute;
    left: 0;
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--accent);
    animation: hs-flow 1.6s linear infinite;
  }
  .hs--still .path i {
    animation: none;
    left: var(--x);
    opacity: 0.5;
  }
  @keyframes hs-flow {
    from {
      left: 0%;
      opacity: 0;
    }
    15% {
      opacity: 1;
    }
    85% {
      opacity: 1;
    }
    to {
      left: 100%;
      opacity: 0;
    }
  }
</style>
