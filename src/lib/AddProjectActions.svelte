<script lang="ts">
  // The Add-project dialog's action bar, including its in-flight state.
  // Cancelling is honest, not a promise: the button says "Stop waiting" for a
  // remote host, and `note` (when the request warrants one) is shown VISIBLY
  // and tied to the button, not hidden in a tooltip.
  let {
    busy,
    stopping,
    remote,
    note,
    canCreate,
    oncreate,
    onclose,
    onstop,
  }: {
    busy: boolean;
    /** The user already asked to stop; waiting for the backend to return. */
    stopping: boolean;
    /** The request in flight targets a remote host. */
    remote: boolean;
    /** What stopping cannot undo for this request, or null. */
    note: string | null;
    canCreate: boolean;
    oncreate: () => void;
    onclose: () => void;
    onstop: () => void;
  } = $props();
</script>

<div class="bar">
  <div class="actions">
    {#if busy}
      <button
        type="button"
        data-testid="cancel-create"
        onclick={onstop}
        disabled={stopping}
        aria-describedby={note ? 'add-inflight-note' : undefined}
      >{stopping ? 'Stopping…' : remote ? 'Stop waiting' : 'Cancel'}</button>
    {:else}
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="button" class="primary" data-testid="add-create" onclick={oncreate} disabled={!canCreate}>Create</button>
    {/if}
  </div>
  {#if busy && note}
    <p class="note" id="add-inflight-note" data-testid="add-inflight-note">{note}</p>
  {/if}
</div>

<style>
  .bar { flex: 0 0 auto; display: flex; flex-direction: column; gap: 0.3rem; padding-top: 0.2rem; border-top: 1px solid var(--border); }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; align-items: center; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button.primary { border-color: var(--accent); }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .note { margin: 0; font-size: 0.75rem; color: var(--fg-muted); text-align: right; }
</style>
