<script lang="ts">
  import { toasts, dismiss } from './toasts';
</script>

<!-- Polite live region: screen readers announce new toasts without
     interrupting; errors stay until dismissed, info auto-clears. -->
<div class="toasts" aria-live="polite" role="status" data-testid="toasts">
  {#each $toasts as t (t.id)}
    <div class="toast {t.kind}" data-testid="toast" data-kind={t.kind}>
      {#if t.code}
        <code class="code" data-testid="toast-code">{t.code}</code>
      {/if}
      <span class="msg">{t.message}</span>
      {#if t.count > 1}
        <span class="count" title="repeated">×{t.count}</span>
      {/if}
      <button class="close" onclick={() => dismiss(t.id)} aria-label="Dismiss" data-testid="toast-dismiss">×</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 0.75rem;
    bottom: 2rem; /* above the 24px status footer */
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
