<script lang="ts">
  // The one dialog pattern (redesign step 5.10, the Dialogs board): a title,
  // one plain sentence, the fields, then Cancel and the verb. Fork, Resume,
  // Review and Send prompt all use it, so the four read alike and a fifth
  // starts from here instead of from a copy of one of them.
  //
  // The sheet owns the frame and the footer only; each dialog keeps its own
  // state, gates and calls. `busy` disables both buttons and swaps the verb
  // for `busyVerb`, with the comet loader in the button (step 3.13): a call
  // is running, never a wait on a person.
  import type { Snippet } from 'svelte';
  import Loader from './Loader.svelte';
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
        onclick={onconfirm}
        >{#if busy}<Loader name="comet" size={12} class="btn-loader" />{busyVerb ?? verb}{:else}{verb}{/if}</button
      >
    </footer>
  </div>
</Modal>

<style>
  footer :global(.btn-loader) {
    margin-right: 0.35em;
    vertical-align: -1px;
  }
  .sheet {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  header {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .sheet-title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--text-lg-weight);
  }
  .sheet-lead {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--text-sm);
    line-height: var(--text-sm-lh);
  }
  .sheet-fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  /* Field rows inside any sheet: a small label over its control. */
  .sheet-fields :global(.field) {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .sheet-fields :global(.field-label) {
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .sheet-fields :global(.field-note) {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .sheet-fields :global(select),
  .sheet-fields :global(textarea),
  .sheet-fields :global(input[type='text']) {
    font: inherit;
    font-size: var(--text-sm);
    color: var(--fg);
    background: var(--bg-sunk);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
  }
  .sheet-fields :global(textarea) {
    resize: vertical;
    font-family: var(--font-mono);
  }
  .sheet-error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .secondary {
    flex: 1;
  }
</style>
