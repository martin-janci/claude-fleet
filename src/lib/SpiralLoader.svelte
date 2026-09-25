<script lang="ts">
  // The spiral loader: a looped squiggle whose stroke runs along it, fast
  // four times then slow twice (see `spiral.ts`). Drawn in `currentColor`, so
  // the caller picks the colour with `color:` — muted for loading, the accent
  // for a turn in progress. Decorative by default; pass `label` when the
  // loader is the only thing saying something is happening.
  //
  // Reduced motion shows one still frame. `paused` does the same without
  // releasing the box, for an indicator that stays mounted while idle.
  import { onMount } from 'svelte';
  import { SPIRAL_PATH, spiralFrame, subscribeSpiralClock } from './spiral';

  let {
    size = 16,
    label,
    paused = false,
    class: klass = '',
    testid = 'spiral-loader',
  }: {
    /** Edge of the square box, in px. */
    size?: number;
    /** Accessible name; without it the loader is hidden from assistive tech. */
    label?: string;
    /** Freeze on the still frame (keeps its box). */
    paused?: boolean;
    class?: string;
    testid?: string;
  } = $props();

  // The still frame: mid-window, fast clip, the squiggle at rest.
  let frame = $state(spiralFrame(0));
  let reduced = $state(false);

  onMount(() => {
    const mq = typeof window.matchMedia === 'function' ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
    reduced = mq?.matches ?? false;
    const onChange = (e: MediaQueryListEvent) => (reduced = e.matches);
    mq?.addEventListener?.('change', onChange);
    return () => mq?.removeEventListener?.('change', onChange);
  });

  $effect(() => {
    if (paused || reduced) {
      frame = spiralFrame(0);
      return;
    }
    let t0: number | null = null;
    return subscribeSpiralClock((now) => {
      if (t0 === null) t0 = now;
      frame = spiralFrame(now - t0);
    });
  });
</script>

<svg
  class="spiral {klass}"
  data-testid={testid}
  width={size}
  height={size}
  viewBox="0 0 16 16"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
  focusable="false"
>
  <path
    d={SPIRAL_PATH}
    transform="translate({frame.x} 8)"
    pathLength="100"
    stroke-dasharray="{frame.end - frame.start} 100"
    stroke-dashoffset={-frame.start}
  />
</svg>

<style>
  .spiral {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
    overflow: hidden;
  }
  path {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
