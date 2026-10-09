<!--
  The first-run tour (Orbit Fleet redesign step 10.5, board Tour): a
  spotlight on one part of the New layout and a popover beside it with the
  step, what the part is for, and "Try it" with the key that uses it. The
  popover never takes the key: the app does what the key does and the
  step ticks "done". Skip, Escape and the last step's Done end it for good.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Button from './kit/Button.svelte';
  import Kbd from './kit/Kbd.svelte';
  import {
    TOUR_STEPS,
    endTour,
    matchesChord,
    nextTourStep,
    placePopover,
    prevTourStep,
    tourStep,
    type Rect,
  } from './tour';

  let { mac = false }: { mac?: boolean } = $props();

  const step = $derived($tourStep === null ? null : TOUR_STEPS[$tourStep]);
  let tried = $state<Record<string, boolean>>({});
  let target = $state<Rect | null>(null);
  let pop = $state<HTMLElement | null>(null);
  let pos = $state<{ left: number; top: number; side: string }>({ left: 0, top: 0, side: 'centre' });

  function findTarget(selectors: readonly string[]): Rect | null {
    for (const sel of selectors) {
      const el = document.querySelector(sel);
      if (!el) continue;
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) return { left: r.left, top: r.top, width: r.width, height: r.height };
    }
    return null;
  }

  function measure() {
    if (!step) return;
    target = findTarget(step.targets);
    const w = pop?.offsetWidth || 320;
    const h = pop?.offsetHeight || 220;
    pos = placePopover(target, { width: w, height: h }, window.innerWidth, window.innerHeight);
  }

  $effect(() => {
    void $tourStep;
    void tick().then(measure);
  });

  onMount(() => {
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  });
  // The layout moves under the tour (panes resize, the inspector opens on
  // ⌥⌘B), so the spotlight follows it, while a tour runs and not otherwise:
  // this component is mounted for the app's life (review r16).
  $effect(() => {
    if ($tourStep === null) return;
    const t = setInterval(measure, 500);
    return () => clearInterval(t);
  });

  function onKeydown(e: KeyboardEvent) {
    if (!step) return;
    if (e.key === 'Escape' && !e.defaultPrevented) {
      endTour();
      return;
    }
    if (matchesChord(e, step.chord, mac)) {
      tried[step.id] = true;
      void tick().then(measure);
    }
  }

  const last = $derived($tourStep === TOUR_STEPS.length - 1);
</script>

<svelte:window onkeydowncapture={onKeydown} />

{#if step && $tourStep !== null}
  {#if target}
    <div
      class="spot"
      aria-hidden="true"
      data-testid="tour-spotlight"
      style="left:{target.left}px;top:{target.top}px;width:{target.width}px;height:{target.height}px"
    ></div>
  {:else}
    <div class="dim" aria-hidden="true"></div>
  {/if}
  <div
    class="pop"
    role="dialog"
    aria-label="Tour"
    data-testid="tour"
    data-side={pos.side}
    bind:this={pop}
    style="left:{pos.left}px;top:{pos.top}px"
  >
    {#if pos.side === 'right' || pos.side === 'left'}<span class="arrow {pos.side}" aria-hidden="true"></span>{/if}
    <div class="meta">Tour · {$tourStep + 1} of {TOUR_STEPS.length}</div>
    <strong class="title">{step.title}</strong>
    <p class="body">{step.body}</p>
    <div class="try" data-testid="tour-try">
      <span class="meta grow">Try it: press <Kbd chord={step.chord} {mac} /> to {step.tryLabel}</span>
      {#if tried[step.id]}<span class="done" data-testid="tour-tried">Done</span>{/if}
    </div>
    <div class="foot">
      <span class="dots" role="img" aria-label="Step {$tourStep + 1} of {TOUR_STEPS.length}">
        {#each TOUR_STEPS as s, i (s.id)}
          <span class="dot" class:past={i < $tourStep} class:now={i === $tourStep}></span>
        {/each}
      </span>
      <span class="grow"></span>
      <Button variant="quiet" size="sm" onclick={endTour} testid="tour-skip">Skip tour</Button>
      <Button variant="quiet" size="sm" disabled={$tourStep === 0} onclick={prevTourStep} label="Previous step" testid="tour-prev"
        >‹</Button
      >
      <Button variant="primary" size="sm" onclick={nextTourStep} testid="tour-next">{last ? 'Done' : 'Next ›'}</Button>
    </div>
  </div>
{/if}

<style>
  .spot,
  .dim {
    position: fixed;
    z-index: 900;
    pointer-events: none;
  }
  .spot {
    border-radius: var(--radius-lg);
    box-shadow: 0 0 0 4000px var(--scrim-strong), 0 0 0 2px var(--accent) inset;
    transition:
      left var(--dur-base),
      top var(--dur-base),
      width var(--dur-base),
      height var(--dur-base);
  }
  .dim {
    inset: 0;
    background: var(--scrim-strong);
  }
  .pop {
    position: fixed;
    z-index: 901;
    width: 320px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-pop);
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .arrow {
    position: absolute;
    top: 28px;
    width: 12px;
    height: 12px;
    background: var(--bg-raise);
    transform: rotate(45deg);
  }
  .arrow.right {
    left: -7px;
    border-left: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }
  .arrow.left {
    right: -7px;
    border-right: 1px solid var(--border);
    border-top: 1px solid var(--border);
  }
  .meta {
    color: var(--fg-muted);
    font-size: var(--text-xs);
  }
  .title {
    font-size: var(--text-md);
  }
  .body {
    margin: 0;
    color: var(--fg-muted);
    line-height: 19px;
  }
  .try {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  .grow {
    flex: 1 1 auto;
  }
  .done {
    color: var(--status-done);
    font-size: var(--text-xs);
  }
  .foot {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .dots {
    display: flex;
    gap: 4px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--border);
  }
  .dot.past {
    background: var(--fg-muted);
  }
  .dot.now {
    background: var(--accent);
  }
</style>
