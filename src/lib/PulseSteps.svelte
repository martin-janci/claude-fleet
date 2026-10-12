<!-- The Pulse sequence tied to a start's real steps (redesign step 5.13,
     LogoMotion and LoadersInUse boards): one satellite per step, lit when
     the step is done and pulsing while it runs, so the mark advances when
     the app hears a step finish, not on a clock. Beside it the steps
     ("Worktree ✓ ready · tmux ✓ up · Claude Code starting") and the caption
     ("Setting up worktree · 1 of 3"). Reduced motion keeps the lit
     satellites and drops the pulse. -->
<script lang="ts">
  import { effectiveMotion } from './motion';
  import type { SessionPulse } from './session_loaders';

  let {
    pulse,
    title = null,
    size = 32,
    markOnly = false,
    testid = 'pulse-steps',
  }: {
    pulse: SessionPulse;
    /** "Starting pd-3011 on mercury"; null for none. */
    title?: string | null;
    size?: number;
    /** Just the mark, for a place that already words the steps. */
    markOnly?: boolean;
    testid?: string;
  } = $props();

  // The mark's three satellites, top then clockwise (the manual's order).
  const SATS = [
    { cx: 54, cy: 30, fill: 'var(--brand-light)' },
    { cx: 74.78, cy: 66, fill: 'var(--brand-amber)' },
    { cx: 33.22, cy: 66, fill: 'var(--brand-light)' },
  ];
  // The Loader kit's rule: Reduced fades the active step slowly, Off holds it.
  const fade = $derived($effectiveMotion === 'reduced');
  const still = $derived($effectiveMotion === 'off');
</script>

<div class="ps" data-testid={testid} data-at={pulse.at} data-done={pulse.done}>
  <svg
    class="mark"
    width={size}
    height={size}
    viewBox="0 0 108 108"
    aria-hidden="true"
  >
    <rect width="108" height="108" rx="24" fill="var(--brand-ink)" />
    <circle cx="54" cy="54" r="24" fill="none" stroke="var(--brand-light)" stroke-opacity="0.5" stroke-width="5" />
    <circle cx="54" cy="54" r="10" fill="var(--brand-light)" />
    {#each pulse.steps as s, i (s.id)}
      {@const sat = SATS[i % SATS.length]}
      <circle
        cx={sat.cx}
        cy={sat.cy}
        r="7.5"
        fill={sat.fill}
        class="sat sat--{s.state}"
        class:sat--fade={fade}
        class:sat--still={still}
        style:transform-origin="{sat.cx}px {sat.cy}px"
        data-testid="pulse-sat"
        data-state={s.state}
      />
    {/each}
  </svg>
  {#if !markOnly}<div class="text">
    {#if title}<span class="title">{title}</span>{/if}
    <span class="caption" role="status" aria-live="polite">{pulse.caption}</span>
    <span class="steps">
      {#each pulse.steps as s (s.id)}
        <span class="step step--{s.state}" data-testid="pulse-step" data-step={s.id} data-state={s.state}
          >{s.label}{#if s.state === 'done'}&nbsp;✓{#if s.note}&nbsp;{s.note}{/if}{:else if s.state === 'active'}&nbsp;starting…{/if}</span
        >
      {/each}
    </span>
  </div>{/if}
</div>

<style>
  .ps {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .mark {
    flex: none;
  }
  .sat {
    transform-box: view-box;
  }
  .sat--pending {
    opacity: 0.25;
  }
  .sat--active {
    animation: ps-pulse var(--loop-fast) ease-in-out infinite;
  }
  .sat--active.sat--fade {
    animation: motion-fade var(--loader-reduced) ease-in-out infinite;
  }
  .sat--active.sat--still {
    animation: none;
    opacity: 0.6;
  }
  @keyframes ps-pulse {
    0%,
    100% {
      transform: scale(1);
      opacity: 1;
    }
    50% {
      transform: scale(1.35);
      opacity: 0.55;
    }
  }
  .text {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .title {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--fg);
  }
  .caption {
    font-size: var(--text-sm);
    color: var(--fg-2);
  }
  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .step--done {
    color: var(--fg-2);
  }
  .step--active {
    color: var(--accent);
  }
</style>
