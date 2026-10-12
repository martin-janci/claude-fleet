<script lang="ts" module>
  import type { LoaderSpec } from './loader-kit-extract';
  import type { LoaderName } from './loader-kit.generated';

  /** The manual's `--loader-delay`. */
  export const LOADER_DELAY_MS = 400;

  /** The one kit loader the Loader board does not draw: moving a session to
   *  another host streams particles from one host to the other (motion.md,
   *  "Move a session to another host"; redesign step 5.13). Drawn here, in
   *  Svelte markup, so it gets the kit's delay, reduced motion, pausing and
   *  off-screen stop like the 24. `HostStream.svelte` puts the hosts beside it. */
  export const HOST_STREAM: LoaderSpec = {
    id: 'host-stream',
    name: 'Host stream',
    kind: 'particle',
    job: 'Moving a session: particles stream from one host to the other',
    width: 120,
    height: 12,
    markup: '',
  };
  export const HOST_STREAM_DOTS = 12;

  export type KitLoaderName = LoaderName | 'host-stream';
</script>

<script lang="ts">
  // The loader kit: the design manual's 24 loaders by name (12 animating the
  // Orbit mark, 12 particle loaders), one job each — see LOADER_SPECS or the
  // manual's Loader board. The drawing and its animation are generated from
  // the manual (loader-kit-extract.ts); this component frames them.
  //
  // It shows nothing for `delay` ms (the manual's `--loader-delay`, 400 ms)
  // so a wait that ends quickly never flashes, but keeps its box so nothing
  // shifts when it appears. Reduced and Off motion (motion.ts) turn every
  // loop into one slow fade; `paused`, an off-screen box or a hidden window
  // stop it without releasing the box. Decorative by default; pass `label`
  // when the loader is the only thing saying something is happening.
  //
  // In a row, a button or the status bar use only `comet` or the 16 px
  // `orbit` (loader-use.test.ts holds every caller to that).
  import { onMount } from 'svelte';
  import { LOADER_SPECS } from './loader-kit.generated';
  import { COUNTED, countedMarkup } from './loader-kit-extract';
  import { effectiveMotion } from './motion';

  let {
    name = 'orbit',
    size,
    label,
    paused = false,
    delay = LOADER_DELAY_MS,
    value,
    count,
    stage,
    class: klass = '',
    testid = 'loader',
  }: {
    name?: KitLoaderName;
    /** Longer edge of the box, in px. Marks 16–48; Comet from 12; particle
     *  loaders default to their natural size on the board. */
    size?: number;
    /** Accessible name; without it the loader is hidden from assistive tech. */
    label?: string;
    /** Freeze it (keeps its box). */
    paused?: boolean;
    /** Ms before it appears; 0 for a stage that already waited. */
    delay?: number;
    /** Progress ring only: 0–1 when the size is known. */
    value?: number;
    /** Radar and Assemble only: one blip or particle per item (hosts that
     *  answered, sessions), up to the kit's cap (`COUNTED`). */
    count?: number;
    /** Dark stage behind it; on by default for particle loaders but Comet. */
    stage?: boolean;
    class?: string;
    testid?: string;
  } = $props();

  const stream = $derived(name === 'host-stream');
  const spec = $derived(stream ? HOST_STREAM : (LOADER_SPECS.find((s) => s.id === name) ?? LOADER_SPECS[0]));
  const comet = $derived(spec.id === 'comet');
  const edge = $derived(size ?? (comet ? 16 : spec.kind === 'logo' ? 32 : Math.max(spec.width, spec.height)));
  // The Comet draws at its size (its ring width follows `--s`); the rest scale.
  const scale = $derived(comet ? 1 : edge / Math.max(spec.width, spec.height));
  const boxW = $derived(comet ? edge : Math.round(spec.width * scale));
  const boxH = $derived(comet ? edge : Math.round(spec.height * scale));
  // The stream sits between two host names, on the page, not on a stage.
  const onStage = $derived(stage ?? (spec.kind === 'particle' && !comet && !stream));
  const determinate = $derived(spec.id === 'progress-ring' && value !== undefined);
  const counted = $derived(count !== undefined && COUNTED[spec.id] !== undefined);
  const markup = $derived(counted ? countedMarkup(spec, count ?? 0) : spec.markup);
  const still = $derived($effectiveMotion !== 'full');

  let shown = $state(false);
  let offscreen = $state(false);
  let windowHidden = $state(typeof document !== 'undefined' && document.hidden);
  let root = $state<HTMLElement | null>(null);
  const frozen = $derived(paused || offscreen || windowHidden);

  $effect(() => {
    if (delay <= 0) {
      shown = true;
      return;
    }
    shown = false;
    const t = setTimeout(() => (shown = true), delay);
    return () => clearTimeout(t);
  });

  onMount(() => {
    const onVisibility = () => (windowHidden = document.hidden);
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  });

  $effect(() => {
    if (!root || typeof IntersectionObserver === 'undefined') return;
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) offscreen = !e.isIntersecting;
    });
    io.observe(root);
    return () => io.disconnect();
  });

  // SMIL (the Atom's orbits) ignores CSS; pause it by hand.
  $effect(() => {
    if (!root || !shown) return;
    const stop = frozen || still;
    for (const svg of Array.from(root.querySelectorAll('svg'))) {
      if (!svg.querySelector('animateMotion, animate, animateTransform')) continue;
      if (stop) svg.pauseAnimations?.();
      else svg.unpauseAnimations?.();
    }
  });
</script>

{#if shown}
  <span
    bind:this={root}
    class="ofl ofl-name-{spec.id} {klass}"
    class:ofl--stage={onStage}
    class:ofl--comet={comet}
    class:ofl--still={still}
    class:ofl--paused={frozen}
    class:ofl--determinate={determinate}
    data-testid={testid}
    data-loader={spec.id}
    data-count={counted ? count : undefined}
    style:width="{boxW}px"
    style:height="{boxH}px"
    style:--s={comet ? `${edge}px` : undefined}
    style:--ofl-p={determinate ? String(Math.min(1, Math.max(0, value ?? 0))) : undefined}
    role={label ? 'img' : undefined}
    aria-label={label}
    aria-hidden={label ? undefined : 'true'}
  >
    {#if comet}
      <!-- Generated from the manual; no user text reaches it. -->
      {@html markup}
    {:else if stream}
      <span
        class="ofl__box ofl-hs"
        style:width="{spec.width}px"
        style:height="{spec.height}px"
        style:transform="scale({scale})"
      >
        {#each Array.from({ length: HOST_STREAM_DOTS }, (_, i) => i) as i (i)}
          <i
            style:animation-delay="calc(var(--loop-slow) * {-i / HOST_STREAM_DOTS})"
            style:top="{(i * 37) % 9}px"
            style:left="{(i * 100) / HOST_STREAM_DOTS}%"
          ></i>
        {/each}
      </span>
    {:else}
      <span
        class="ofl__box"
        style:width="{spec.width}px"
        style:height="{spec.height}px"
        style:transform="scale({scale})"
      >
        <!-- Generated from the manual; no user text reaches it. -->
        {@html markup}
      </span>
    {/if}
  </span>
{:else}
  <span
    class="ofl ofl--pending {klass}"
    data-testid="{testid}-pending"
    style:width="{boxW}px"
    style:height="{boxH}px"
    aria-hidden="true"
  ></span>
{/if}
