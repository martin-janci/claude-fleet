<script lang="ts">
  import { toasts, droppedToasts, dismiss, clearToasts, runToastAction } from './toasts';
</script>

<!-- Polite live region: screen readers announce new toasts without
     interrupting; errors stay until dismissed, info auto-clears. -->
<div class="toasts" aria-live="polite" role="status" data-testid="toasts">
  {#each $toasts as t (t.id)}
    <!-- No nested live region: an assertive role="alert" inside this polite
         role="status" is undefined behaviour and screen readers either
         double-announce it or drop one. The container announces. -->
    <div class="toast {t.kind}" data-testid="toast" data-kind={t.kind}>
      {#if t.code}
        <code class="code" data-testid="toast-code">{t.code}</code>
      {/if}
      <span class="msg">{t.message}</span>
      {#if t.action}
        <button class="action" onclick={() => runToastAction(t.id)} data-testid="toast-action">{t.action.label}</button>
      {/if}
      {#if t.count > 1}
        <span class="count" title="repeated">×{t.count}</span>
      {/if}
      <button class="close" onclick={() => dismiss(t.id)} aria-label="Dismiss" data-testid="toast-dismiss">×</button>
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
    font-size: 0.7rem;
    color: var(--fg-muted);
  }
  .dismiss-all {
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 4px;
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
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-left-width: 3px;
    border-radius: 6px;
    background: var(--bg);
    color: var(--fg);
    font-size: 0.8rem;
    line-height: 1.35;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.18);
  }
  .toast.error { border-left-color: #e64a4a; }
  .toast.success { border-left-color: #50c86e; }
  .toast.info { border-left-color: var(--accent); }
  .code {
    flex: 0 0 auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    background: var(--bg-pane);
    color: var(--fg-muted);
  }
  .msg { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  .count { color: var(--fg-muted); font-size: 0.7rem; }
  .action {
    flex: 0 0 auto;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--accent);
    cursor: pointer;
    font-size: 0.75rem;
    padding: 0 0.4rem;
  }
  .action:hover { border-color: var(--accent); }
  .close {
    flex: 0 0 auto;
    background: transparent;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 1rem;
    line-height: 1;
    padding: 0 0.1rem;
  }
  .close:hover { color: var(--fg); }
</style>
