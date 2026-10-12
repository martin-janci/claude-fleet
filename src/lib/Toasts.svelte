<script lang="ts">
  import {
    toasts,
    droppedToasts,
    dismiss,
    clearToasts,
    runToastAction,
    toastCountdowns,
    holdToast,
    releaseToast,
  } from './toasts';
  import { effectiveMotion } from './motion';
  import Loader from './Loader.svelte';
  import { timerBarRuns, toastIn, toastOut } from './motion_catalog';
</script>

<!-- Polite live region: screen readers announce new toasts without
     interrupting; errors stay until dismissed, info auto-clears. -->
<div class="toasts" aria-live="polite" role="status" data-testid="toasts">
  {#each $toasts as t (t.id)}
    <!-- No nested live region: an assertive role="alert" inside this polite
         role="status" is undefined behaviour and screen readers either
         double-announce it or drop one. The container announces. -->
    <!-- Redesign step 7.4: enters from the right, leaves with a fade; the
         countdown holds while the pointer or focus is on the toast. -->
    {@const countdown = $toastCountdowns[t.id]}
    <div
      class="toast {t.kind}"
      data-testid="toast"
      data-kind={t.kind}
      in:toastIn
      out:toastOut
      onmouseenter={() => holdToast(t.id)}
      onmouseleave={() => releaseToast(t.id)}
      onfocusin={() => holdToast(t.id, 'focus')}
      onfocusout={() => releaseToast(t.id, 'focus')}
      role="group"
    >
      {#if t.progress !== undefined}
        <!-- Step 10.10: a long job's toast carries its Progress ring; the
             toast's action opens the job. -->
        <span
          class="job"
          role="progressbar"
          aria-label={t.message}
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={Math.round(t.progress * 100)}
        >
          <Loader name="progress-ring" size={28} value={t.progress} delay={0} testid="toast-ring" />
        </span>
      {/if}
      <span class="msg"
        >{t.message}{#if t.sub}<span class="sub" data-testid="toast-sub">{t.sub}</span>{/if}{#if t.code}<details class="details"
            ><summary>Details</summary><code class="code" data-testid="toast-code">{t.detail ?? t.code}</code></details
          >{/if}</span
      >
      {#if t.action}
        <!-- The Toasts board's two-button toast: the answer first, then the
             quiet way out ("Add rule" · "Not now"). -->
        <span class="actions">
          <button class="action" onclick={() => runToastAction(t.id)} data-testid="toast-action">{t.action.label}</button>
          {#if t.secondary}
            <button class="action action--quiet" onclick={() => runToastAction(t.id, 'secondary')} data-testid="toast-secondary"
              >{t.secondary.label}</button
            >
          {/if}
        </span>
      {/if}
      {#if t.count > 1}
        <span class="count" title="repeated">×{t.count}</span>
      {/if}
      <button class="close" onclick={() => dismiss(t.id)} aria-label="Dismiss" data-testid="toast-dismiss">×</button>
      {#if countdown && timerBarRuns($effectiveMotion)}
        {#key countdown.arm}
          <span
            class="timer"
            data-testid="toast-timer"
            aria-hidden="true"
            style:animation-duration="{countdown.ms}ms"
          ></span>
        {/key}
      {/if}
    </div>
  {/each}
  <!-- LAST, not first. The column is anchored at its bottom edge and grows
       upward, so its first child is the one a tall stack pushes off the top of
       the viewport — which is exactly when the escape hatch is needed. Sitting
       next to the anchor, it is on screen no matter how many toasts are up. -->
  {#if $toasts.length > 1 || $droppedToasts > 0}
    <div class="bar">
      {#if $droppedToasts > 0}
        <!-- The cap threw some away; saying "Dismiss all (5)" and nothing else
             would understate what actually went wrong. -->
        <span class="dropped" data-testid="toast-dropped">+{$droppedToasts} older not shown</span>
      {/if}
      <button class="dismiss-all" onclick={() => clearToasts()} data-testid="toast-dismiss-all">
        Dismiss all ({$toasts.length})
      </button>
    </div>
  {/if}
</div>

<style>
  /* Errors are sticky, so N failing sessions leave N toasts and the only
     way out was N individual × clicks. */
  .bar {
    pointer-events: auto;
    align-self: flex-end;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .dropped {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .dismiss-all {
    font: inherit;
    font-size: var(--text-2xs);
    padding: 0.15rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-pane);
    color: var(--fg-muted);
    cursor: pointer;
  }
  .dismiss-all:hover { color: var(--fg); border-color: var(--accent); }
  .toasts {
    position: fixed;
    right: 0.75rem;
    /* Above the status footer AND the agent FAB: a toast must never cover
       the app's only documented entry point to the agent. */
    bottom: calc(var(--status-h) + var(--fab-size) + var(--layer-gap) * 2);
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    z-index: 50;
    max-width: min(420px, calc(100vw - 1.5rem));
    pointer-events: none;
  }
  .toast {
    position: relative;
    overflow: hidden;
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-left-width: 3px;
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--fg);
    font-size: var(--text-2xs);
    line-height: 1.35;
    box-shadow: var(--shadow-pop);
  }
  .toast.error { border-left-color: var(--danger); }
  .toast.success { border-left-color: var(--status-done); }
  .toast.warning { border-left-color: var(--status-waiting); }
  .toast.info { border-left-color: var(--accent); }
  .details {
    margin-top: 0.2rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .details summary {
    cursor: pointer;
  }
  .code {
    display: block;
    margin-top: 0.15rem;
    overflow-wrap: anywhere;
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
    background: var(--bg-pane);
    color: var(--fg-muted);
  }
  .msg { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .sub {
    display: block;
    margin-top: 0.1rem;
    color: var(--fg-muted);
  }
  .actions {
    flex: 0 0 auto;
    display: inline-flex;
    gap: 0.3rem;
  }
  .count { color: var(--fg-muted); font-size: var(--text-2xs); }
  .job {
    display: inline-flex;
    flex: none;
  }
  .action {
    flex: 0 0 auto;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--accent);
    cursor: pointer;
    font-size: var(--text-2xs);
    padding: 0 0.4rem;
  }
  .action:hover { border-color: var(--accent); }
  .action--quiet { color: var(--fg-muted); }
  .action--quiet:hover { color: var(--fg); border-color: var(--border); }
  .close {
    flex: 0 0 auto;
    background: transparent;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: var(--text-md);
    line-height: 1;
    padding: 0 0.1rem;
  }
  .close:hover { color: var(--fg); }
  /* The countdown of an auto-dismissing toast: it drains left to right and
     stops while the toast is hovered or focused, as the timer does. */
  .timer {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: var(--fg-muted);
    opacity: 0.45;
    transform-origin: left;
    animation-name: toast-drain;
    animation-timing-function: linear;
    animation-fill-mode: forwards;
  }
  .toast:hover .timer,
  .toast:focus-within .timer {
    animation-play-state: paused;
  }
  @keyframes toast-drain {
    from { transform: scaleX(1); }
    to { transform: scaleX(0); }
  }
</style>
