<script lang="ts">
  // The one dialog pattern (redesign step 5.10, the Dialogs board): a title,
  // one plain sentence, the fields, then Cancel and the verb. Fork, Resume,
  // Review and Send prompt all use it, so the four read alike and a fifth
  // starts from here instead of from a copy of one of them.
  //
  // The sheet owns the frame and the footer only; each dialog keeps its own
  // state, gates and calls. `busy` disables both buttons and swaps the verb
  // for `busyVerb` (no spinner: the loader rules keep spinners out of a
  // dialog that is waiting on a person, and this one is waiting on a call
  // that names itself).
  import type { Snippet } from 'svelte';
  import Modal from './Modal.svelte';

  let {
    title,
    lead,
    verb,
    busyVerb,
    onconfirm,
    onclose,
    canConfirm = true,
    busy = false,
    error = null,
    width = '480px',
    testid,
    confirmTestid,
    errorTestid,
    confirmTitle = null,
    secondary,
    children,
  }: {
    title: string;
    /** The one plain sentence under the title. */
    lead: string;
    /** The confirm button's label: the action, never "OK". */
    verb: string;
    busyVerb?: string;
    onconfirm: () => void;
    onclose: () => void;
    canConfirm?: boolean;
    busy?: boolean;
    /** Shown above the footer, in the danger color. */
    error?: string | null;
    width?: string;
    testid?: string;
    confirmTestid?: string;
    errorTestid?: string;
    /** Why the verb is off, as its tooltip (a gate's reason). */
    confirmTitle?: string | null;
    /** A quiet action at the footer's start (e.g. "Start fresh instead"). */
    secondary?: Snippet;
    children: Snippet;
  } = $props();
</script>

<Modal label={title} onclose={busy ? undefined : onclose} {width} {testid}>
  <div class="sheet">
    <header>
      <h3 class="sheet-title">{title}</h3>
      <p class="sheet-lead">{lead}</p>
    </header>

    <div class="sheet-fields">
      {@render children()}
    </div>

    {#if error}
      <p class="sheet-error" role="alert" data-testid={errorTestid}>{error}</p>
    {/if}

    <footer>
      <div class="secondary">{#if secondary}{@render secondary()}{/if}</div>
      <button type="button" class="btn" onclick={onclose} disabled={busy} data-testid="sheet-cancel">Cancel</button>
      <button
        type="button"
        class="btn btn--primary"
        data-testid={confirmTestid}
        title={confirmTitle ?? ''}
        disabled={!canConfirm || busy}
        onclick={onconfirm}>{busy ? (busyVerb ?? verb) : verb}</button
      >
    </footer>
  </div>
</Modal>

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    gap: var(--space-3, 12px);
  }
  header {
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
  }
  .sheet-title {
    margin: 0;
    font-size: var(--text-lg, 15px);
    font-weight: var(--text-lg-weight, 600);
  }
  .sheet-lead {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-sm, 12.5px);
    line-height: var(--text-sm-lh, 1.45);
  }
  .sheet-fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3, 12px);
  }
  /* Field rows inside any sheet: a small label over its control. */
  .sheet-fields :global(.field) {
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
  }
  .sheet-fields :global(.field-label) {
    font-size: var(--text-xs, 11.5px);
    color: var(--fg-muted);
  }
  .sheet-fields :global(.field-note) {
    margin: 0;
    font-size: var(--text-xs, 11.5px);
    color: var(--fg-muted);
  }
  .sheet-fields :global(select),
  .sheet-fields :global(textarea),
  .sheet-fields :global(input[type='text']) {
    font: inherit;
    font-size: var(--text-sm, 12.5px);
    color: var(--fg);
    background: var(--bg-sunk, var(--bg-pane));
    border: 1px solid var(--control-border, var(--border));
    border-radius: var(--radius-sm, 4px);
    padding: var(--space-1, 4px) var(--space-2, 8px);
  }
  .sheet-fields :global(textarea) {
    resize: vertical;
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .sheet-error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm, 12.5px);
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--space-2, 8px);
  }
  .secondary {
    flex: 1;
  }
</style>
