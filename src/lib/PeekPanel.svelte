<script lang="ts">
  let { tmuxName, peek, onClose }: { tmuxName: string; peek: string; onClose: () => void } =
    $props();
</script>

<div class="peek-panel" data-testid="peek-panel">
  <div class="peek-header">
    <span>Logs — {tmuxName}</span>
    <button onclick={() => onClose()} class="peek-close">✕</button>
  </div>
  {#if peek === "loading"}
    <p class="peek-loading">Loading…</p>
  {:else}
    <pre class="peek-output">{peek}</pre>
  {/if}
</div>

<style>
  .peek-panel {
    background: var(--color-surface-2, #1e1e2e);
    border: 1px solid var(--border);
    border-radius: 4px;
    margin: 2px 8px 4px 8px;
    padding: 8px;
    font-size: 12px;
  }
  .peek-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
    font-weight: 600;
  }
  .peek-close {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--fg-muted);
  }
  .peek-loading {
    color: var(--fg-muted);
    font-style: italic;
    margin: 0;
  }
  .peek-output {
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow-y: auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    margin: 0;
  }
</style>
