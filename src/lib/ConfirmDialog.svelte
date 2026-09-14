<script lang="ts">
  import type { Snippet } from 'svelte';
  import Modal from './Modal.svelte';

  // Yes/no question on top of Modal. `message` is plain text; pass `children`
  // instead (or as well) when the body needs markup such as <code>name</code>.
  let {
    title,
    message,
    confirmLabel = 'OK',
    cancelLabel = 'Cancel',
    danger = false,
    busy = false,
    onconfirm,
    oncancel,
    confirmTestId,
    children,
  }: {
    title: string;
    message?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    /** Destructive action: red confirm button, initial focus on Cancel. */
    danger?: boolean;
    /** Disables both buttons while the action is in flight. */
    busy?: boolean;
    onconfirm: () => void;
    oncancel: () => void;
    confirmTestId?: string;
    children?: Snippet;
  } = $props();
</script>

<Modal {title} onclose={oncancel} width="380px" testid="confirm-dialog">
  {#if message}
    <p class="message">{message}</p>
  {/if}
  {#if children}
    <div class="message">{@render children()}</div>
  {/if}
  <div class="actions">
    <button
      onclick={oncancel}
      disabled={busy}
      data-autofocus={danger ? '' : undefined}
      data-testid="confirm-cancel"
    >{cancelLabel}</button>
    <button
      class:danger
      class:primary={!danger}
      onclick={onconfirm}
      disabled={busy}
      data-autofocus={danger ? undefined : ''}
      data-testid={confirmTestId}
    >{confirmLabel}</button>
  </div>
</Modal>

<style>
  .message {
    margin: 0;
    font-size: 0.85rem;
    color: var(--fg-muted);
    line-height: 1.4;
  }
  .message :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg-pane);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    color: var(--fg);
  }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
  .actions button.danger { color: #e64a4a; border-color: #e64a4a; }
  .actions button.danger:hover:not(:disabled) { background: rgba(230, 74, 74, 0.12); }
</style>
